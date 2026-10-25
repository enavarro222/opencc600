//! The ride being recorded (crates/recorder), and the finished rides on the rides screen.

use embassy_time::{Duration, Instant};
use littlefs2::io::Error;
use log::{info, warn};
use nrf_link::command;
use ride_store::room;
use status::Status;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::config::{settings, utc_offset};
use crate::{Fs, serial};
use crate::nrf::{SOUND_BEEP, sound};
use crate::pairing::sensor_values;

/// What the screen shows of the ride.
#[derive(Clone, Copy, Default, PartialEq)]
#[allow(
    clippy::large_stack_frames,
    reason = "Default of a 528-byte struct (the altitude history), built once a frame on the 215 KB main stack"
)]
pub(crate) struct RideInfo {
    /// Started/resumed by the rider, including an automatic pause.
    pub(crate) running: bool,
    pub(crate) auto_paused: bool,
    pub(crate) timer_s: u32,
    pub(crate) elapsed_s: u32,
    pub(crate) distance_cm: u32,
    pub(crate) ascent_m: u16,
    pub(crate) descent_m: u16,
    pub(crate) max_speed_mm_s: u32,
    pub(crate) avg_power_w: Option<u16>,
    pub(crate) avg_heart_rate: Option<u8>,
    pub(crate) max_heart_rate: Option<u8>,
    /// Altitude along the ride (gradient, profiles).
    pub(crate) history: ui::history::History,
    /// The ride is no longer written.
    pub(crate) failure: Option<ui::WriteFailure>,
    /// A point every 5 s: the storage is nearly full.
    pub(crate) sparse: bool,
}

/// Why a ride did not start.
pub(crate) enum StartError {
    /// No GNSS fix nor RTC time yet.
    NoTime,
    /// Less than [`room::RIDE_START_BYTES`] free, without the logs.
    Full,
    /// Another littlefs error.
    Storage(i32),
}

/// The ride being recorded: one FIT record per GNSS epoch (1 Hz) while the timer runs.
pub(crate) struct Ride {
    pub(crate) recorder: recorder::Recorder,
    odometer: recorder::Odometer,
    /// GNSS time of the last epoch recorded.
    last_epoch: Option<gnss::Time>,
    clock: recorder::EpochClock,
    last_timestamp: u32,
    /// Altitude along the ride, in RAM (lost when the ride is resumed after a power off).
    history: ui::history::History,
    /// The last write failed even with the logs' room: when, and why. Nothing is written before [`RETRY_WRITE`].
    failure: Option<(Instant, ui::WriteFailure)>,
    /// When the room left was read last (`None`: at the next sample): below [`room::SPARSE_BYTES`] the ride records
    /// a point every [`recorder::SPARSE_INTERVAL_S`], to its end.
    room_read: Option<Instant>,
}

/// Room for the rides: free, and what the logs take (they give way).
pub(crate) fn ride_room(fs: &Fs<'_>) -> littlefs2::io::Result<usize> {
    Ok(room::free_bytes(fs)? + room::logs_bytes(fs)?)
}

/// A finished ride on the rides screen.
pub(crate) struct StoredRide {
    pub(crate) id: u32,
    pub(crate) sent: bool,
    totals: Option<fit::Totals>,
}

/// The finished rides, in the order of the rides screen: the latest first; to make room, the ones the app has first,
/// then the oldest first.
pub(crate) fn stored_rides(fs: &Fs<'_>, making_room: bool) -> Vec<StoredRide> {
    let t = Instant::now();
    let mut list = Vec::new();
    if let Err(e) = ride_store::finished_sent(fs, |id, size, sent| list.push((id, size, sent))) {
        warn!("rides: littlefs error {} listing", e.code());
    }
    let mut rides: Vec<StoredRide> = list
        .into_iter()
        .map(|(id, size, sent)| StoredRide { id, sent, totals: recorder::finished_totals(fs, id, size).ok() })
        .collect();
    if making_room {
        rides.sort_by_key(|r| (!r.sent, r.id));
    } else {
        rides.sort_by_key(|r| core::cmp::Reverse(r.id));
    }
    info!("rides: {} listed in {} ms", rides.len(), t.elapsed().as_millis());
    rides
}

pub(crate) fn ride_card(r: &StoredRide) -> ui::RideCard {
    let local = nrf_link::RtcTime::from_unix(i64::from(r.id) + i64::from(utc_offset(Some(r.id))));
    let t = r.totals.unwrap_or_default();
    ui::RideCard {
        weekday: local.weekday(),
        day: local.day,
        month: local.month,
        hour: local.hour,
        minute: local.minute,
        timer_s: t.timer_ms / 1000,
        distance_cm: t.distance_cm,
        sent: r.sent,
    }
}

/// After a failed write (full storage), the next try waits this long.
pub(crate) const RETRY_WRITE: Duration = Duration::from_secs(60);

impl Ride {
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handles (cache included) on the main stack, once per ride"
    )]
    pub(crate) fn start(fs: &Fs<'_>, s: &Status) -> Result<Box<Self>, StartError> {
        let Some(unix) = s.now_unix() else {
            warn!("ride: no time yet (no GNSS fix, no RTC), not started");
            return Err(StartError::NoTime);
        };
        // the logs give way to the rides
        match room::for_ride(fs) {
            Ok(free) if free < room::RIDE_START_BYTES => {
                warn!("ride: {free} bytes free without the logs, not started");
                return Err(StartError::Full);
            }
            Ok(_) => {}
            Err(e) => {
                warn!("ride: littlefs error {} making room, not started", e.code());
                return Err(StartError::Storage(e.code()));
            }
        }
        match recorder::Recorder::start(fs, unix, fit::timestamp(unix), serial()) {
            Ok(recorder) => {
                info!("ride {unix}: started");
                sound(SOUND_BEEP);
                Ok(Box::new(Self {
                    recorder,
                    odometer: recorder::Odometer::default(),
                    last_epoch: None,
                    clock: recorder::EpochClock::default(),
                    last_timestamp: fit::timestamp(unix),
                    history: ui::history::History::new(),
                    failure: None,
                    room_read: None,
                }))
            }
            Err(e) => {
                warn!("ride: start failed, littlefs error {}", e.code());
                Err(if e == Error::NO_SPACE { StartError::Full } else { StartError::Storage(e.code()) })
            }
        }
    }

    /// Ride `id` opened again, paused: interrupted (`finished` false) or finished (the end screen's "resume").
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handles (cache included) on the main stack, once per ride"
    )]
    pub(crate) fn reopen(fs: &Fs<'_>, id: u32, finished: bool) -> Option<Box<Self>> {
        let res = if finished {
            recorder::Recorder::reopen_finished(fs, id)
        } else {
            recorder::Recorder::reopen(fs, id)
        };
        match res {
            Ok(recorder) => {
                let t = recorder.summary().totals();
                info!("ride {id}: reopened paused, {t:?}");
                Some(Box::new(Self {
                    odometer: recorder::Odometer::starting_at(t.distance_cm),
                    last_epoch: None,
                    clock: recorder::EpochClock::default(),
                    last_timestamp: t.end_time,
                    history: ui::history::History::new(),
                    failure: None,
                    room_read: None,
                    recorder,
                }))
            }
            Err(e) => {
                warn!("ride {id}: reopen failed, littlefs error {}", e.code());
                None
            }
        }
    }

    /// Writes what is buffered, before a power off: the ride (paused) is offered again at power-on.
    pub(crate) fn keep(&mut self, fs: &Fs<'_>) {
        let res = self.recorder.flush(fs);
        self.wrote(fs, res);
        if self.failure.is_none() {
            info!("ride {}: kept for the next power-on", self.recorder.id());
        }
    }

    /// Follows a write: on a full storage the logs' room is taken and what is buffered written again. If that fails
    /// too (or on another error), the ride is not written for [`RETRY_WRITE`] (the screen says so).
    pub(crate) fn wrote(&mut self, fs: &Fs<'_>, res: littlefs2::io::Result<()>) {
        let mut res = res;
        if res == Err(Error::NO_SPACE) {
            let t = Instant::now();
            match room::for_ride(fs) {
                Ok(free) => {
                    info!("ride: storage full, {free} bytes free without the logs ({} ms)", t.elapsed().as_millis())
                }
                Err(e) => warn!("ride: littlefs error {} making room", e.code()),
            }
            res = self.recorder.flush(fs);
        }
        match (res, self.failure) {
            (Ok(()), None) => {}
            (Ok(()), Some(_)) => {
                info!("ride: written again");
                self.failure = None;
            }
            (Err(e), _) => {
                warn!(
                    "ride: write failed, littlefs error {}, not tried again for {} s",
                    e.code(),
                    RETRY_WRITE.as_secs()
                );
                let why = if e == Error::NO_SPACE { ui::WriteFailure::Full } else { ui::WriteFailure::Error };
                self.failure = Some((Instant::now(), why));
            }
        }
    }

    pub(crate) fn toggle_pause(&mut self, fs: &Fs<'_>, s: &Status) {
        let t = s.now_unix()
            .map_or(self.last_timestamp, fit::timestamp)
            .max(self.last_timestamp);
        let running = self.recorder.active();
        let res = if running {
            self.recorder.pause(fs, t)
        } else {
            self.recorder.resume(fs, t)
        };
        self.wrote(fs, res);
        self.last_timestamp = t;
        // the timer follows the key anyway: only the event may be missing from the file
        info!("ride: {}", if running { "paused" } else { "resumed" });
        if self.failure.is_none() {
            sound(SOUND_BEEP);
        }
    }

    /// Records the latest GNSS epoch if it is new.
    #[allow(
        clippy::large_stack_frames,
        reason = "a fix and a record, once a second from main (215 KB stack)"
    )]
    pub(crate) fn sample(&mut self, fs: &Fs<'_>, s: &Status) {
        // Elapsed time keeps advancing even without a new GNSS epoch, using the RTC when needed.
        if let Some(timestamp) = s.now_unix().map(fit::timestamp) {
            self.last_timestamp = self.last_timestamp.max(timestamp);
        }
        let Some(fix) = s.fix else { return };
        if fix.time.is_none() || fix.time == self.last_epoch {
            return;
        }
        self.last_epoch = fix.time;
        let Some(epoch) = fix.time else { return };
        let epoch_s = epoch.hour as u32 * 3600 + epoch.minute as u32 * 60 + epoch.second as u32;
        let gnss = s.gnss_unix().map(fit::timestamp);
        let rtc = s.rtc_unix().map(fit::timestamp);
        let Some(timestamp) = self.clock.next(epoch_s, gnss, rtc) else {
            return;
        };
        let timestamp = timestamp.max(self.last_timestamp);
        self.last_timestamp = timestamp;
        let sensors = sensor_values();
        let speed = sensors.speed_mm_s.or(fix.valid.then_some(fix.speed_mm_s).flatten());
        let config = settings();
        let was_paused = self.recorder.auto_paused();
        let res = self.recorder.auto_pause(fs, timestamp, speed, config.auto_pause.then_some(config.auto_pause_kmh));
        if self.recorder.auto_paused() != was_paused || res.is_err() {
            self.wrote(fs, res);
        }
        if self.recorder.auto_paused() != was_paused {
            info!("ride: auto-{}", if self.recorder.auto_paused() { "paused" } else { "resumed" });
        }
        let min_speed = if config.auto_pause { (u32::from(config.auto_pause_kmh) * 1000).div_ceil(36) } else { recorder::MOVING_MM_S };
        let distance_cm = self.odometer.update_with_min_speed(timestamp, speed, self.recorder.summary().running(), min_speed);
        let record = fit::Record {
            timestamp,
            lat_e7: fix.lat_e7,
            lon_e7: fix.lon_e7,
            // the barometer calibrated on the GNSS; the raw GNSS altitude until then, or without a barometer
            altitude_dm: s
                .altimeter
                .altitude_dm()
                .or(fix.valid.then_some(fix.alt_dm).flatten()),
            speed_mm_s: speed,
            distance_cm: Some(distance_cm),
            temperature_c: s.baro.map(|(c, _)| (c / 100) as i8),
            pressure_pa: s.baro.map(|(_, p)| (p + 50) / 100),
            heart_rate: sensors.heart_rate,
            cadence: sensors.cadence_rpm,
            power_w: sensors.power_w,
        };
        // a point every 5 s once the storage is nearly full (a walk through the file system: once a minute)
        if self.recorder.interval_s() == 1 && self.room_read.is_none_or(|t| t.elapsed() >= RETRY_WRITE) {
            self.room_read = Some(Instant::now());
            if let Ok(left) = ride_room(fs)
                && left < room::SPARSE_BYTES
            {
                info!("ride: {left} bytes left for the rides, a point every {} s", recorder::SPARSE_INTERVAL_S);
                self.recorder.set_interval(recorder::SPARSE_INTERVAL_S);
            }
        }
        // after a failure, what is buffered is written first, every [`RETRY_WRITE`]; the records meanwhile are lost
        let failing = match self.failure {
            Some((at, _)) if at.elapsed() < RETRY_WRITE => true,
            Some(_) => {
                let res = self.recorder.flush(fs);
                self.wrote(fs, res);
                self.failure.is_some()
            }
            None => false,
        };
        if !failing {
            let res = self.recorder.record(fs, &record);
            self.wrote(fs, res);
        }
        if let (true, Some(alt)) = (self.recorder.summary().running(), record.altitude_dm) {
            self.history.push(distance_cm, alt);
            self.history.push_time(self.recorder.summary().totals().timer_ms / 1000, alt);
        }
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handles and the recorder on the main stack, once per ride"
    )]
    pub(crate) fn stop(mut self: Box<Self>, fs: &Fs<'_>, s: &Status) {
        let id = self.recorder.id();
        // the room for what is buffered and the closing messages
        let res = self.recorder.flush(fs);
        self.wrote(fs, res);
        let t = s.now_unix()
            .map_or(self.last_timestamp, fit::timestamp)
            .max(self.last_timestamp);
        match self.recorder.stop(fs, t, utc_offset(s.now_unix())) {
            Ok(totals) => {
                info!("ride {id}: saved, {totals:?}");
                sound(command::SOUND_JINGLE);
            }
            Err(e) => warn!(
                "ride {id}: stop failed, littlefs error {} (closed at next boot)",
                e.code()
            ),
        }
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "returns a RideInfo (528 bytes), once a frame from main (215 KB stack)"
    )]
    pub(crate) fn info(&self) -> RideInfo {
        let t = self.recorder.summary().totals_at(self.last_timestamp);
        RideInfo {
            running: self.recorder.active(),
            auto_paused: self.recorder.auto_paused(),
            timer_s: t.timer_ms / 1000,
            elapsed_s: t.elapsed_ms / 1000,
            distance_cm: self.odometer.distance_cm(),
            ascent_m: t.ascent_m,
            descent_m: t.descent_m,
            max_speed_mm_s: t.max_speed_mm_s,
            avg_power_w: self.recorder.summary().avg_power_w(),
            avg_heart_rate: self.recorder.summary().avg_heart_rate(),
            max_heart_rate: self.recorder.summary().max_heart_rate(),
            history: self.history,
            failure: self.failure.map(|(_, why)| why),
            sparse: self.recorder.interval_s() > 1,
        }
    }
}
