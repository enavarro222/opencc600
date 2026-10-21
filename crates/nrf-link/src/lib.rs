//! UART link between the CC600 ESP32-S3 and its nRF co-processor (UART2, 115200 8N1).
//!
//! The nRF owns the keys, power, charging, RTC, the pressure / light sensors and ANT+. Reference:
//! c606-oss `docs/HARDWARE.md` ("nRF co-processor link") and `main/nrf_link.c`.
//!
//! Frame: `A5 N 6F DIR TYPE CMD payload[N-4] CRClo CRChi`, total `N + 4` bytes, CRC-16/XMODEM over everything
//! before the CRC. Hardware independent: [`Decoder`] takes the received bytes, [`command`] builds the frames to send.

#![no_std]

pub const SYNC: u8 = 0xA5;
pub const MARK: u8 = 0x6F;
/// `DIR` byte of ESP32 → nRF frames (not checked by the vendor on nRF frames).
pub const DIR_ESP_TO_NRF: u8 = 0xF1;
/// The vendor rejects `N >= 0x85`.
pub const MAX_N: u8 = 0x84;
pub const MAX_FRAME: usize = MAX_N as usize + 4;
pub const MAX_PAYLOAD: usize = MAX_N as usize - 4;

pub const TYPE_QUERY: u8 = 1;
pub const TYPE_SET: u8 = 2;

pub const CMD_SYS: u8 = 0x10;

/// CRC-16/XMODEM: poly 0x1021, init 0, no reflection, no final xor.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    PayloadTooLong,
    BufferTooSmall,
}

/// Writes an ESP32 → nRF frame into `out`, returns its length.
pub fn encode(kind: u8, cmd: u8, payload: &[u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(EncodeError::PayloadTooLong);
    }
    let total = payload.len() + 8;
    if out.len() < total {
        return Err(EncodeError::BufferTooSmall);
    }
    out[..6].copy_from_slice(&[
        SYNC,
        payload.len() as u8 + 4,
        MARK,
        DIR_ESP_TO_NRF,
        kind,
        cmd,
    ]);
    out[6..6 + payload.len()].copy_from_slice(payload);
    let crc = crc16(&out[..total - 2]);
    out[total - 2..total].copy_from_slice(&crc.to_le_bytes());
    Ok(total)
}

/// A received frame whose header and CRC have been checked.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    buf: [u8; MAX_FRAME],
    len: u8,
}

impl Frame {
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }
    pub fn kind(&self) -> u8 {
        self.buf[4]
    }
    pub fn cmd(&self) -> u8 {
        self.buf[5]
    }
    pub fn payload(&self) -> &[u8] {
        &self.buf[6..self.len as usize - 2]
    }
}

impl core::fmt::Debug for Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Frame(")?;
        for (i, b) in self.bytes().iter().enumerate() {
            write!(f, "{}{b:02X}", if i == 0 { "" } else { " " })?;
        }
        write!(f, ")")
    }
}

/// Byte-stream framer, same acceptance rules as the vendor receive task: `A5`, `N < 0x85`, `6F`, type 1..=5,
/// valid CRC. Anything else drops one byte and resyncs.
#[derive(Clone)]
pub struct Decoder {
    buf: [u8; MAX_FRAME],
    len: usize,
    /// Frames dropped because of a bad CRC (resync count).
    pub crc_errors: u32,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_FRAME],
            len: 0,
            crc_errors: 0,
        }
    }

    /// Feeds received bytes; `on_frame` is called for every complete, valid frame.
    pub fn feed(&mut self, data: &[u8], mut on_frame: impl FnMut(Frame)) {
        for &b in data {
            self.buf[self.len] = b;
            self.len += 1;
            while let Some(frame) = self.next_frame() {
                on_frame(frame);
            }
        }
    }

    fn drop_front(&mut self, n: usize) {
        self.buf.copy_within(n..self.len, 0);
        self.len -= n;
    }

    fn next_frame(&mut self) -> Option<Frame> {
        loop {
            let b = &self.buf[..self.len];
            let header_ok = !b.is_empty()
                && b[0] == SYNC
                && b.get(1).is_none_or(|&n| (4..=MAX_N).contains(&n))
                && b.get(2).is_none_or(|&m| m == MARK)
                && b.get(4).is_none_or(|&t| (1..=5).contains(&t));
            if !header_ok {
                if self.len == 0 {
                    return None;
                }
                self.drop_front(1);
                continue;
            }
            if b.len() < 2 {
                return None;
            }
            let total = b[1] as usize + 4;
            if b.len() < total {
                return None;
            }
            let want = u16::from_le_bytes([b[total - 2], b[total - 1]]);
            if crc16(&b[..total - 2]) != want {
                self.crc_errors += 1;
                self.drop_front(1);
                continue;
            }
            let mut frame = Frame {
                buf: [0; MAX_FRAME],
                len: total as u8,
            };
            frame.buf[..total].copy_from_slice(&b[..total]);
            self.drop_front(total);
            return Some(frame);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Short press, sent on release.
    Click,
    /// Long press: first frame ~0.5 s after press-down, then repeated every ~250 ms while held.
    LongPress,
    /// Release after a long press.
    LongRelease,
    Other(u8),
}

/// Time of the nRF RTC (UTC as written by the vendor firmware).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcTime {
    pub year: u16,
    /// 1..=12
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl RtcTime {
    /// Seconds since 1970-01-01 00:00:00 (no leap seconds, like UTC on the receivers).
    pub fn unix(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
            + self.second as i64
    }

    /// Inverse of [`RtcTime::unix`].
    pub fn from_unix(unix: i64) -> Self {
        let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (secs / 3600) as u8,
            minute: (secs / 60 % 60) as u8,
            second: (secs % 60) as u8,
        }
    }

    /// 0 = Sunday, the value [`command::set_rtc`] expects.
    pub fn weekday(&self) -> u8 {
        // 1970-01-01 was a Thursday
        (days_from_civil(self.year, self.month, self.day) + 4).rem_euclid(7) as u8
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn days_from_civil(y: u16, m: u8, d: u8) -> i64 {
    let (y, m) = if m <= 2 {
        (y as i64 - 1, m as i64 + 9)
    } else {
        (y as i64, m as i64 - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Date of a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (u16, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = yoe + era * 400 + (m <= 2) as i64;
    (y as u16, m, d)
}

/// [`Message::Status`] values, read in the ride logs (`charger` column): 0 plugged in, charging or charged (the
/// voltage rising, or steady at 100 %), 1 on battery (falling).
pub const USB_POWERED: u8 = 0;
pub const ON_BATTERY: u8 = 1;

/// nRF → ESP32 messages known so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// Key index 0..=2 (the three physical keys).
    Key {
        key: u8,
        action: KeyAction,
    },
    /// Every 300 ms.
    Battery {
        millivolts: u16,
        percent: u8,
    },
    /// Every 300 ms: the power source, [`USB_POWERED`] or [`ON_BATTERY`].
    Status(u8),
    Rtc(RtcTime),
    /// Reply to a power-on query: 4 = manual power-on, 5/6 = charger.
    PowerOnReason(u8),
    /// Acknowledge of an `E2` control command.
    Ack,
    Version {
        major: u8,
        minor: u8,
        patch: u8,
    },
    /// Sensor stream (5 Hz, only after a long press of the power key): raw accelerometer, ±8 g full scale.
    /// Sent by the C606 nRF only, never seen on the CC600 (same for `Gyro`).
    Accel([i16; 3]),
    /// Raw gyro, ±2000 dps full scale.
    Gyro([i16; 3]),
    /// Barometer: temperature in 0.01 °C, pressure in 0.01 Pa.
    Baro {
        centi_celsius: i16,
        centi_pascal: u32,
    },
    /// Ambient light sensor (CC600, 3.3 Hz; checked by covering / lighting the device). The nRF cycles through
    /// measurements tagged by `param`: 600 (`level` ~1000 indoors at night, ~2500 by day: the ambient light),
    /// then 2000 and 3000 (`level` < 10 indoors; meaning unknown, maybe another channel or gain).
    /// Vendor factory test strings: `Environment Light:`, `LightLux:`; used for the `Auto(Light)` backlight.
    Light {
        level: u16,
        param: u32,
    },
    /// An ANT+ channel's state (`17 <type> <number low byte> 00 <transmission> <state> 00 00`, type 4 cmd 0x01): one
    /// of [`ant::CONNECTED`], [`ant::NOT_CONNECTED`], [`ant::SEARCH_TIMEOUT`]. Sent on changes and, for each
    /// channel the nRF knows, every 5 s (c606-oss `docs/HARDWARE.md`; the CC600's seen during a search, type 0).
    AntChannel {
        device_type: u8,
        number_low: u8,
        transmission: u8,
        state: u8,
    },
    /// A sensor heard by an ANT+ search (`F3 03 <type> <number LE> <transmission> <rssi> <flag>`, type 4 cmd 0x10);
    /// `end` (flag 0, zeros otherwise, seen on the CC600): the search is over.
    AntFound {
        device_type: u8,
        number: u16,
        transmission: u8,
        rssi_dbm: i8,
        end: bool,
    },
    /// An ANT+ data page from an open channel (type 4, the device type as the cmd byte), ~4 a second.
    AntPage {
        device_type: u8,
        page: [u8; 8],
    },
    /// Other sensor stream subtype, not understood yet (CC600: `0A`, 3.3 Hz, tracks the barometer temperature).
    Stream {
        sub: u8,
        data: [u8; 6],
    },
    Unknown,
}

/// ANT+ through the nRF: it is only the radio, the ESP32 opens the channels and decodes the pages (c606-oss
/// `main/ant.c`, reverse engineered from the vendor's `AntSendCmd` / `AntPageEventHandler`).
pub mod ant {
    /// [`super::Message::AntChannel`] states.
    pub const CONNECTED: u8 = 3;
    /// Also the periodic report of a channel the nRF knows: not a reason to reopen it (that drops a live one).
    pub const NOT_CONNECTED: u8 = 4;
    /// The search of an open channel gave up ([`super::command::ant_open`]'s timeout): open it again.
    pub const SEARCH_TIMEOUT: u8 = 5;
    /// The device types the nRF sends pages for (as the frame's cmd byte): power, fitness equipment, shifting,
    /// light, radar, heart rate, speed and cadence, cadence, speed, Shimano Di2.
    pub const DEVICE_TYPES: [u8; 10] = [11, 17, 34, 35, 40, 120, 121, 122, 123, 128];
    /// Sub-command of the channel commands and states (cmd 0x01).
    pub const CHANNEL: u8 = 0x17;
    /// Search result (cmd 0x10).
    pub const FOUND: u8 = 0xF3;
}

fn u16_at(p: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(p.get(i..i + 2)?.try_into().ok()?))
}

fn i16x3_at(p: &[u8], i: usize) -> Option<[i16; 3]> {
    let v = |k: usize| u16_at(p, i + 2 * k).map(|x| x as i16);
    Some([v(0)?, v(1)?, v(2)?])
}

impl Message {
    pub fn parse(frame: &Frame) -> Message {
        Self::try_parse(frame).unwrap_or(Message::Unknown)
    }

    fn try_parse(frame: &Frame) -> Option<Message> {
        let p = frame.payload();
        Some(match (frame.cmd(), *p.first()?) {
            (0x00, 0x52) => Message::Battery {
                millivolts: u16_at(p, 4)?,
                percent: *p.get(6)?,
            },
            (0x00, 0x53) => {
                let t = p.get(2..8)?;
                Message::Rtc(RtcTime {
                    second: t[0],
                    minute: t[1],
                    hour: t[2],
                    day: t[3] & 0x1F,
                    month: t[4] + 1,
                    year: 1900 + u16::from(t[5]),
                })
            }
            (0x01, 0x01) => Message::Version {
                major: *p.get(7)?,
                minor: *p.get(8)?,
                patch: *p.get(9)?,
            },
            (CMD_SYS, 0x49) => Message::Key {
                key: *p.get(1)?,
                action: match *p.get(6)? {
                    1 => KeyAction::Click,
                    4 => KeyAction::LongPress,
                    5 => KeyAction::LongRelease,
                    v => KeyAction::Other(v),
                },
            },
            // the nRF acknowledges every command with its first bytes (`E2 02 01`, `E1 02 01`, `00 00` for ANT)
            _ if frame.kind() == 5 => Message::Ack,
            (0x01, ant::CHANNEL) => Message::AntChannel {
                device_type: *p.get(1)?,
                number_low: *p.get(2)?,
                transmission: *p.get(4)?,
                state: *p.get(5)?,
            },
            (CMD_SYS, ant::FOUND) if p.get(1) == Some(&3) => Message::AntFound {
                device_type: *p.get(2)?,
                number: u16_at(p, 3)?,
                transmission: *p.get(5)?,
                rssi_dbm: *p.get(6)? as i8,
                end: *p.get(7)? == 0,
            },
            (t, _) if frame.kind() == 4 && ant::DEVICE_TYPES.contains(&t) => Message::AntPage {
                device_type: t,
                page: p.get(..8)?.try_into().ok()?,
            },
            (CMD_SYS, 0xE2) if p.get(1) == Some(&2) => Message::PowerOnReason(*p.get(5)?),
            (CMD_SYS, 0xF0) if p.get(1) == Some(&2) => Message::Status(*p.get(2)?),
            (CMD_SYS, 0xF1) => match *p.get(1)? {
                0x01 => Message::Accel(i16x3_at(p, 2)?),
                0x02 => Message::Gyro(i16x3_at(p, 2)?),
                0x03 => Message::Baro {
                    centi_celsius: u16_at(p, 2)? as i16,
                    centi_pascal: u32::from_le_bytes(p.get(4..8)?.try_into().ok()?),
                },
                0x04 => Message::Light {
                    param: u32::from_le_bytes(p.get(2..6)?.try_into().ok()?),
                    level: u16_at(p, 6)?,
                },
                sub => Message::Stream {
                    sub,
                    data: p.get(2..8)?.try_into().ok()?,
                },
            },
            _ => return None,
        })
    }
}

/// ESP32 → nRF commands, as complete frames.
pub mod command {
    use super::*;

    /// Length of a control frame: 8-byte payload `E2 grp id 00 00 val 00 00`.
    pub const CTRL_FRAME: usize = 16;

    /// Any `E2 grp id 00 00 val 00 00` set command (type 2), for exploring the nRF from the test hook.
    pub fn e2(grp: u8, id: u8, val: u8) -> [u8; CTRL_FRAME] {
        ctrl(TYPE_SET, grp, id, val)
    }

    /// Any frame with an 8-byte payload, for exploring the nRF from the test hook.
    pub fn raw(kind: u8, cmd: u8, payload: [u8; 8]) -> [u8; CTRL_FRAME] {
        let mut out = [0; CTRL_FRAME];
        encode(kind, cmd, &payload, &mut out).unwrap();
        out
    }

    /// ANT+ search for `seconds` (0 stops), vendor `StartAntScanDrive` (c606-oss `ant_scan`): the nRF answers
    /// with `F3 03 xx <type> <num LE> <trans> <rssi> <flag>` frames (type 4, cmd 0x10), flag 0 at the end.
    pub fn ant_scan(seconds: u16) -> [u8; CTRL_FRAME] {
        let [lo, hi] = seconds.to_le_bytes();
        raw(TYPE_SET, CMD_SYS, [0xE1, 0x02, 0x00, lo, hi, 0xFF, 0x00, 0x00])
    }

    /// Opens an ANT+ channel to a sensor (vendor `AntSnd:2,1,P=17 …`): the nRF searches for it up to
    /// [`ANT_SEARCH_S`], then answers [`ant::CONNECTED`] or [`ant::SEARCH_TIMEOUT`]. Number 0: any sensor of the type.
    /// Not within ~6 s of a [`power_on`] command at boot: the nRF kills it at its next 5 s report, and doing so
    /// again and again left its ANT stack silent until the three-key reset (c606-oss).
    pub fn ant_open(device_type: u8, number: u16, transmission: u8) -> [u8; CTRL_FRAME] {
        let [lo, hi] = number.to_le_bytes();
        raw(TYPE_SET, 0x01, [ant::CHANNEL, device_type, lo, hi, transmission, 0x00, ANT_SEARCH_S, 0x00])
    }

    /// The vendor's search timeout, seconds.
    pub const ANT_SEARCH_S: u8 = 25;

    /// Closes the channel of a device type (vendor `StartAntDisConnectDrive`): only to pair another sensor. Sent at
    /// boot it left the nRF's ANT stack silent on a C606 (c606-oss).
    pub fn ant_close(device_type: u8) -> [u8; CTRL_FRAME] {
        raw(TYPE_SET, 0x01, [ant::CHANNEL, device_type, 0, 0, 0, 0x01, 0, 0])
    }

    /// Sends a data page to the sensor on the channel of a device type (vendor `FUN_420e3430`, the power meter
    /// calibration `Start Calib send`: `kind 2, cmd = device type, the page`).
    pub fn ant_send(device_type: u8, page: [u8; 8]) -> [u8; CTRL_FRAME] {
        raw(TYPE_SET, device_type, page)
    }

    fn ctrl(kind: u8, grp: u8, id: u8, val: u8) -> [u8; CTRL_FRAME] {
        let mut out = [0; CTRL_FRAME];
        encode(kind, CMD_SYS, &[0xE2, grp, id, 0, 0, val, 0, 0], &mut out).unwrap();
        out
    }

    /// Vendor `SendPowerOnCmd`: sent at boot until the nRF replies (`E2 02` reply).
    pub fn power_on() -> [u8; CTRL_FRAME] {
        ctrl(TYPE_SET, 0x02, 0x00, 0x01)
    }

    /// Vendor `SendCheckPowerOnReasonCmd`: answered by [`Message::PowerOnReason`].
    pub fn check_power_on_reason() -> [u8; CTRL_FRAME] {
        ctrl(TYPE_QUERY, 0x02, 0x00, 0x01)
    }

    /// Vendor `SendPowerOffCmd`: the nRF cuts the ESP32 power.
    pub fn power_off() -> [u8; CTRL_FRAME] {
        ctrl(TYPE_SET, 0x02, 0x00, 0x00)
    }

    /// Sets the nRF RTC: the RTC frame it sends (`cmd 00`, `'S'`), as a type 2 (set) frame (CC600, checked).
    /// `weekday`: 0 = Sunday, in the 3 high bits of the day byte like the nRF's own frames. Unlike them, the month
    /// is 1..12 here (writing month - 1 moved the RTC back one month).
    pub fn set_rtc(t: &RtcTime, weekday: u8) -> [u8; CTRL_FRAME] {
        let mut out = [0; CTRL_FRAME];
        let payload = [
            0x53,
            0xFF,
            t.second,
            t.minute,
            t.hour,
            t.day | (weekday << 5),
            t.month,
            (t.year - 1900) as u8,
        ];
        encode(TYPE_SET, 0x00, &payload, &mut out).unwrap();
        out
    }

    /// GNSS module power: 0 off, 1 on, 2 (vendor value, meaning unknown).
    pub fn gps_power(val: u8) -> [u8; CTRL_FRAME] {
        ctrl(TYPE_SET, 0x02, 0x07, val)
    }

    /// [`misc`] value playing a short melody ("tou-di-dou") on the buzzer (CC600, checked by ear).
    pub const SOUND_JINGLE: u8 = 0x17;

    /// `E2 01 XX 00 00 00 00 00`, "LED/misc" in c606-oss; the vendor sends XX < 0x1C (0x13, 0x17, 0x18, 0x19 seen).
    /// Each is acknowledged. On the CC600, [`SOUND_JINGLE`] plays a short melody on the buzzer; 0x13, 0x18 and 0x19
    /// make no sound; the other values are untested.
    pub fn misc(id: u8) -> [u8; CTRL_FRAME] {
        ctrl(TYPE_SET, 0x01, id, 0x00)
    }
}
