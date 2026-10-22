//! The trip log: the raw sensors in a CSV file, when asked for (`set trip_log on`).

use embassy_time::{Duration, Instant};
use log::{info, warn};
use opencc600_firmware::{ant, ble};
use ride_store::room;
use status::Status;

use alloc::format;
use alloc::string::String;

use crate::{Fs, STATUS};
use crate::nrf::KEY_LOG;
use crate::ride::{RETRY_WRITE, RideInfo};

/// Raw sensor log, when asked for (`set trip_log on`): one CSV line per GNSS epoch (or every 2 s if the receiver is
/// silent), appended every 10 s to `/logs/<unix>.csv`, a new file every [`room::LOG_FILE_BYTES`]. The logs give
/// way to the rides ([`room`]): the oldest are deleted, and the log stops when there is no room. For replaying the
/// altitude filter and the GNSS on a PC, and a first battery-life record. Contains real positions: copy it to
/// `dump/` only.
pub(crate) struct TripLog {
    /// The file written, `None` until there is room for a new one.
    path: Option<littlefs2::path::PathBuf>,
    /// Written into `path`.
    file_bytes: usize,
    buf: String,
    last_epoch: Option<gnss::Time>,
    last_line: Instant,
    last_flush: Instant,
    started: Instant,
    start_unix: u32,
    lines: u32,
    /// The last write or the room for a new file failed: nothing tried before [`RETRY_WRITE`] (on a full storage,
    /// each try walks the whole file system).
    failed_at: Option<Instant>,
    /// The free room read last (the rides fill it too): below the reserve, the file is closed.
    room_read: Instant,
}

/// What the trip log writes a second (measured: 3.3 MB for a 5 h session).
pub(crate) const TRIP_LOG_BYTES_PER_S: u32 = 190;

pub(crate) const TRIP_LOG_HEADER: &str = "unix,gnss_unix,rtc_unix,uptime_s,fix,quality,sats_used,sats_in_view,hdop_c,lat_e7,\
lon_e7,gnss_alt_dm,speed_mm_s,course_cdeg,baro_c_celsius,pressures_cpa,altitude_dm,battery_mv,battery_pct,charger,\
light,ride,gnss_bytes,gnss_silences,gnss_setups,gnss_restarts,gnss_aiding_requests,gnss_time_aids,gnss_ttff_ms,\
keys,ble_links,ble_listening,ant_links\n";

impl TripLog {
    #[allow(clippy::large_stack_frames, reason = "the log (a littlefs path) built on the main stack, once")]
    pub(crate) fn start(s: &Status) -> Option<Self> {
        let start_unix = s.now_unix()?;
        let now = Instant::now();
        Some(Self {
            path: None,
            file_bytes: 0,
            buf: String::new(),
            last_epoch: None,
            last_line: now,
            last_flush: now,
            started: now,
            start_unix,
            lines: 0,
            failed_at: None,
            room_read: now,
        })
    }

    /// A new file, with its header, once there is room for it.
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle on the main stack, once a log file"
    )]
    pub(crate) fn new_file(&self, fs: &Fs<'_>) -> Option<littlefs2::path::PathBuf> {
        let t = Instant::now();
        match room::for_log(fs) {
            Ok(true) => {}
            Ok(false) => {
                warn!("trip log: no room out of the rides' reserve, off for now");
                return None;
            }
            Err(e) => {
                warn!("trip log: littlefs error {} making room", e.code());
                return None;
            }
        }
        let unix = self.start_unix + self.started.elapsed().as_secs() as u32;
        let path = room::log_path(unix);
        let res = if fs.exists(room::LOGS) { Ok(()) } else { fs.create_dir(room::LOGS) };
        if let Err(e) = res.and_then(|()| fs.write(&path, TRIP_LOG_HEADER.as_bytes())) {
            warn!("trip log: littlefs error {} creating /logs/{unix}.csv", e.code());
            return None;
        }
        info!("trip log: /logs/{unix}.csv ({} ms to make room)", t.elapsed().as_millis());
        Some(path)
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle and a Status copy on the main stack, once a second at most"
    )]
    pub(crate) fn sample(&mut self, fs: &Fs<'_>, s: &Status, ride: Option<RideInfo>) {
        let epoch = s.fix.and_then(|f| f.time);
        let new_epoch = epoch.is_some() && epoch != self.last_epoch;
        if !new_epoch && self.last_line.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_epoch = epoch;
        self.last_line = Instant::now();
        let pressures = STATUS.lock(|c| {
            let mut st = c.get();
            let p = (st.baro_recent, st.baro_recent_n);
            st.baro_recent_n = 0;
            c.set(st);
            p
        });
        let opt = |v: Option<i64>| v.map_or_else(String::new, |v| format!("{v}"));
        let f = s.fix.unwrap_or_default();
        let mut p = String::new();
        for (i, v) in pressures.0[..pressures.1 as usize].iter().enumerate() {
            if i > 0 {
                p.push(';');
            }
            p.push_str(&format!("{v}"));
        }
        let line = format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            opt(s.now_unix().map(i64::from)),
            opt(s.gnss_unix().map(i64::from)),
            opt(s.rtc_unix().map(i64::from)),
            self.started.elapsed().as_secs(),
            f.valid as u8,
            f.quality,
            f.sats_used,
            f.sats_in_view,
            opt(f.hdop_c.map(i64::from)),
            opt(f.lat_e7.map(i64::from)),
            opt(f.lon_e7.map(i64::from)),
            opt(f.alt_dm.map(i64::from)),
            opt(f.speed_mm_s.map(i64::from)),
            opt(f.course_cdeg.map(i64::from)),
            opt(s.baro.map(|(c, _)| i64::from(c))),
            p,
            opt(s.altimeter.altitude_dm().map(i64::from)),
            opt(s.battery.map(|(mv, _)| i64::from(mv))),
            opt(s.battery.map(|(_, pct)| i64::from(pct))),
            opt(s.charger.map(i64::from)),
            opt(s.light[0].map(i64::from)),
            match ride {
                None => 0,
                Some(r) if r.running => 1,
                Some(_) => 2,
            },
            s.gnss.map_or(0, |g| g.bytes),
            s.gnss_link.silences,
            s.gnss_link.setups,
            s.gnss_link.restarts,
            s.gnss_link.aiding_requests,
            s.gnss_link.time_aids,
            opt(s.gnss_link.ttff_ms.map(i64::from)),
            KEY_LOG.lock(|l| core::mem::take(&mut *l.borrow_mut())),
            ble::links(),
            ble::listening() as u8,
            ant::links(),
        );
        self.buf.push_str(&line);
        self.lines += 1;
        if self.last_flush.elapsed() >= Duration::from_secs(10) || self.buf.len() > 2048 {
            self.flush(fs);
        }
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle and path on the main stack, every 10 s"
    )]
    pub(crate) fn flush(&mut self, fs: &Fs<'_>) {
        self.last_flush = Instant::now();
        if self.failed_at.is_some_and(|t| t.elapsed() < RETRY_WRITE) {
            if self.buf.len() > 16 * 1024 {
                self.buf.clear();
            }
            return;
        }
        self.failed_at = None;
        // a walk through the file system (~30 ms): once a minute
        if self.path.is_some() && self.room_read.elapsed() >= RETRY_WRITE {
            self.room_read = Instant::now();
            if room::free_bytes(fs).is_ok_and(|free| free < room::RESERVE_BYTES) {
                info!("trip log: in the rides' reserve, a new file once there is room");
                self.path = None;
            }
        }
        if self.path.is_none() {
            self.path = self.new_file(fs);
            self.file_bytes = 0;
        }
        let Some(path) = self.path.as_ref() else {
            self.failed_at = Some(Instant::now());
            return;
        };
        let res = fs.open_file_with_options_and_then(
            |o| o.write(true).append(true),
            path,
            |file| {
                let mut rest = self.buf.as_bytes();
                while !rest.is_empty() {
                    let n = file.write(rest)?;
                    rest = &rest[n..];
                }
                Ok(())
            },
        );
        match res {
            Ok(()) => {
                if self.lines % 60 < 10 {
                    info!("trip log: {} lines", self.lines);
                }
                self.file_bytes += self.buf.len();
                self.buf.clear();
                if self.file_bytes >= room::LOG_FILE_BYTES {
                    self.path = None;
                }
            }
            Err(e) => {
                // storage full or broken: keep running, a new file (room made first) in a while
                warn!("trip log: littlefs error {}, tried again in {} s", e.code(), RETRY_WRITE.as_secs());
                self.failed_at = Some(Instant::now());
                self.path = None;
                if self.buf.len() > 16 * 1024 {
                    self.buf.clear();
                }
            }
        }
    }
}
