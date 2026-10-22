//! The device's state: the latest of what the nRF sends (battery, charger, RTC, keys, barometer, light) and of the
//! GNSS receiver (fix, link counters), and the time now that follows from them. Written by the firmware's nRF and
//! GNSS tasks, read by the main loop. Times are milliseconds of a monotonic clock (the firmware's uptime).

#![no_std]

use nrf_link::{KeyAction, Message, RtcTime};

/// A fix older than this is no longer valid (the receiver gives one a second).
pub const FIX_MAX_AGE_MS: u64 = 3_000;

/// GNSS receiver events since boot, for the trip log and the time-to-first-fix measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GnssLink {
    /// 3 s without data (receiver off or DMA stopped), each answered by `gps_power(1)`.
    pub silences: u16,
    /// Output setups sent (at boot, then after each receiver restart).
    pub setups: u16,
    /// `PAIR011,001` seen: the receiver (re)started.
    pub restarts: u16,
    /// `PAIR010` aiding requests (EPO, time, position).
    pub aiding_requests: u16,
    /// `PAIR590` time aiding sent.
    pub time_aids: u16,
    /// Time to the first valid fix since the last start (ESP32 boot, restart seen, or commanded), ms.
    pub ttff_ms: Option<u32>,
}

impl GnssLink {
    pub const fn new() -> Self {
        Self {
            silences: 0,
            setups: 0,
            restarts: 0,
            aiding_requests: 0,
            time_aids: 0,
            ttff_ms: None,
        }
    }
}

impl Default for GnssLink {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Status {
    pub battery: Option<(u16, u8)>,
    pub charger: Option<u8>,
    pub rtc: Option<RtcTime>,
    pub version: Option<(u8, u8, u8)>,
    pub power_on_reason: Option<u8>,
    pub last_key: Option<(u8, KeyAction)>,
    pub keys: u32,
    pub baro: Option<(i16, u32)>,
    /// Barometer altitude, its bias calibrated on the GNSS (docs/ALTITUDE.md); fed by each pressure frame (5 Hz)
    /// and each GNSS epoch.
    pub altimeter: altitude::Altimeter,
    baro_at_ms: Option<u64>,
    /// Pressures (centi-pascals) received since the trip log took them (5 Hz frames, logged once a second).
    pub baro_recent: [u32; 8],
    pub baro_recent_n: u8,
    /// Ambient light (raw) per measurement tag (`param` 600, 2000, 3000), and the raw sensor stream subtype `0A`
    /// (meaning unknown).
    pub light: [Option<u16>; 3],
    pub stream0a: Option<[u8; 6]>,
    /// Storage partition: Ok((finished rides, free blocks, total blocks)) or the littlefs error code.
    pub storage: Option<Result<(u16, u16, u16), i32>>,
    /// GNSS link counters and latest complete fix.
    pub gnss: Option<gnss::Stats>,
    pub gnss_link: GnssLink,
    pub fix: Option<gnss::Fix>,
    /// When `fix` was last set: see [`Status::expire_fix`].
    fix_at_ms: Option<u64>,
}

impl Default for Status {
    fn default() -> Self {
        Self::new()
    }
}

impl Status {
    pub const fn new() -> Self {
        Self {
            battery: None,
            charger: None,
            rtc: None,
            version: None,
            power_on_reason: None,
            last_key: None,
            keys: 0,
            baro: None,
            altimeter: altitude::Altimeter::new(altitude::Tuning::DEFAULT),
            baro_at_ms: None,
            baro_recent: [0; 8],
            baro_recent_n: 0,
            light: [None; 3],
            stream0a: None,
            storage: None,
            gnss: None,
            gnss_link: GnssLink::new(),
            fix: None,
            fix_at_ms: None,
        }
    }

    /// A message of the nRF, received at `now_ms`.
    pub fn update(&mut self, msg: Message, now_ms: u64) {
        match msg {
            Message::Battery { millivolts, percent } => self.battery = Some((millivolts, percent)),
            Message::Status(s) => self.charger = Some(s),
            Message::Rtc(t) => self.rtc = Some(t),
            Message::Version { major, minor, patch } => self.version = Some((major, minor, patch)),
            Message::PowerOnReason(r) => self.power_on_reason = Some(r),
            Message::Key { key, action } => {
                self.last_key = Some((key, action));
                self.keys += 1;
            }
            Message::Baro { centi_celsius, centi_pascal } => {
                self.baro = Some((centi_celsius, centi_pascal));
                let dt = self.baro_at_ms.map_or(0.0, |at| now_ms.saturating_sub(at) as f64 / 1e3);
                self.baro_at_ms = Some(now_ms);
                self.altimeter.baro(dt, centi_pascal as f64 / 100.0);
                if (self.baro_recent_n as usize) < self.baro_recent.len() {
                    self.baro_recent[self.baro_recent_n as usize] = centi_pascal;
                    self.baro_recent_n += 1;
                }
            }
            Message::Light { level, param } => {
                if let Some(i) = [600, 2000, 3000].iter().position(|&p| p == param) {
                    self.light[i] = Some(level);
                }
            }
            Message::Stream { sub: 0x0A, data } => self.stream0a = Some(data),
            Message::Ack
            | Message::Accel(_)
            | Message::Gyro(_)
            | Message::Stream { .. }
            | Message::AntChannel { .. }
            | Message::AntFound { .. }
            | Message::AntPage { .. }
            | Message::Unknown => {}
        }
    }

    /// The fix of an epoch of the receiver, or a position sent over USB, at `now_ms`.
    pub fn set_fix(&mut self, fix: gnss::Fix, now_ms: u64) {
        self.fix = Some(fix);
        self.fix_at_ms = Some(now_ms);
    }

    /// A fix without a new epoch for [`FIX_MAX_AGE_MS`] (the receiver silent, the positions sent over USB stopped)
    /// is no longer valid: the time comes from the RTC again, the speed and the position are gone. True if it
    /// changed.
    pub fn expire_fix(&mut self, now_ms: u64) -> bool {
        match (self.fix.as_mut(), self.fix_at_ms) {
            (Some(f), Some(at)) if f.valid && now_ms.saturating_sub(at) >= FIX_MAX_AGE_MS => {
                f.valid = false;
                true
            }
            _ => false,
        }
    }

    /// Current time, Unix seconds: the GNSS time with a valid fix, else the nRF RTC.
    pub fn now_unix(&self) -> Option<u32> {
        self.gnss_unix().or_else(|| self.rtc_unix())
    }

    /// GNSS time, only with a valid fix (without, it can be off by tens of seconds).
    pub fn gnss_unix(&self) -> Option<u32> {
        let fix = self.fix?;
        let (true, Some(time), Some(date)) = (fix.valid, fix.time, fix.date) else {
            return None;
        };
        unix_of(&RtcTime {
            year: date.year,
            month: date.month,
            day: date.day,
            hour: time.hour,
            minute: time.minute,
            second: time.second,
        })
    }

    pub fn rtc_unix(&self) -> Option<u32> {
        unix_of(&self.rtc?)
    }
}

/// A time a FIT file can hold (after its epoch, 1989-12-31): an RTC never set starts at 2000 or earlier.
fn unix_of(t: &RtcTime) -> Option<u32> {
    u32::try_from(t.unix()).ok().filter(|&u| u > fit::FIT_EPOCH_OFFSET)
}
