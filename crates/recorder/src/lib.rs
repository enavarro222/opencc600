//! Ride recording on [`ride_store`], hardware independent.
//!
//! A ride is a FIT activity file written as it goes: a placeholder header, the definitions, `file_id`, a timer
//! start, then [`Record`]s buffered in RAM and appended every [`FLUSH_EVERY_S`] seconds (at most that much is lost
//! on a crash). [`Recorder::stop`] appends the closing messages (timer stop, lap, session, activity), then
//! [`seal`]s the file: the real header and the file CRC, written in one littlefs commit, and the rename to `.fit`.
//!
//! The totals ([`Summary`]) are computed from the FIT messages themselves, as a decoder reads them back: live while
//! recording, and by replaying the file in [`recover`] after a power cut. Both give the same session.

#![no_std]

use fit::{
    Activity, Crc, Event, EventType, FileId, HEADER_LEN, Lap, Message, Record, Session, Totals,
};
use ride_store::littlefs2::driver::Storage;
use ride_store::littlefs2::fs::Filesystem;
use ride_store::littlefs2::io::{Error, Result, SeekFrom};
use ride_store::room;

/// Bytes a ride takes a second: one [`Record`] (the closing messages are small next to it), ~122 KB an hour.
pub const RIDE_BYTES_PER_S: u32 = Record::DEFINITION.data_len() as u32;

/// With less room than [`room::SPARSE_BYTES`] for the rides, one record every this many seconds
/// ([`Recorder::set_interval`]), ~40 h of riding in the last 1 MB instead of ~8 h.
pub const SPARSE_INTERVAL_S: u32 = 5;

/// Hours of riding the free space holds, with `extra_bytes_per_s` written alongside (the trip log, which stays out
/// of the last [`room::SPARSE_BYTES`], ridden with a record every [`SPARSE_INTERVAL_S`]); a tenth is kept for the
/// file system (its metadata, the blocks half filled).
pub fn hours_left(free_bytes: u64, extra_bytes_per_s: u32) -> u32 {
    let usable = free_bytes * 9 / 10;
    let sparse = usable.min(room::SPARSE_BYTES as u64);
    let seconds = (usable - sparse) / (RIDE_BYTES_PER_S + extra_bytes_per_s) as u64
        + sparse * SPARSE_INTERVAL_S as u64 / RIDE_BYTES_PER_S as u64;
    (seconds / 3600) as u32
}

/// Appends at most this often (seconds of ride time): each append is a littlefs commit (a block copy).
pub const FLUSH_EVERY_S: u32 = 10;
/// RAM buffer: a flush period of records (34 bytes each) plus events, and the file start (~230 bytes).
const BUF: usize = 384;
/// Altitude hysteresis for the ascent / descent totals, decimetres.
///
/// The totals count the pressure altitude of the records (`absolute_pressure`), not their altitude: the altitude
/// is the barometer with a bias calibrated on the GNSS, whose corrections would count as climbing
/// (docs/ALTITUDE.md). No pressure, no ascent: never from the GNSS altitude.
pub const CLIMB_HYSTERESIS_DM: i32 = 20;
/// Faster than this a pressure altitude cannot rise on a bike (a strong rider climbs ~0.5 m/s), decimetres a second:
/// such a rise is held, not counted, until it falls back (a pressure wave: a train in a tunnel, a door slammed) or
/// [`SPIKE_HOLD_S`] goes by (it was real: a lift, a car). Falls are never held: a fast descent counts at once.
pub const MAX_CLIMB_DM_S: i32 = 15;
/// A held rise still there after this long counts.
pub const SPIKE_HOLD_S: u32 = 60;

/// Totals over the ride, fed with the messages of the file in order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    start_time: Option<u32>,
    last_time: u32,
    /// Timer running since (FIT timestamp), None while paused.
    running_since: Option<u32>,
    timer_s: u32,
    distance_cm: u32,
    max_speed_mm_s: u32,
    climb_ref_dm: Option<i32>,
    /// The last pressure altitude (FIT timestamp, dm), and a rise held: the altitude before it, since when.
    last_alt: Option<(u32, i32)>,
    held: Option<(i32, u32)>,
    ascent_dm: u32,
    descent_dm: u32,
    /// Sum and count of the powers recorded (0 included: coasting counts, as Garmin's default), and of the heart
    /// rates.
    power: (u64, u32),
    heart_rate: (u64, u32),
    pub records: u32,
}

impl Summary {
    pub fn event(&mut self, e: &Event) {
        self.start_time.get_or_insert(e.timestamp);
        self.last_time = self.last_time.max(e.timestamp);
        match e.kind {
            EventType::Start => {
                self.running_since.get_or_insert(e.timestamp);
            }
            EventType::Stop | EventType::StopAll => {
                if let Some(since) = self.running_since.take() {
                    self.timer_s += e.timestamp.saturating_sub(since);
                }
            }
        }
    }

    pub fn record(&mut self, r: &Record) {
        self.start_time.get_or_insert(r.timestamp);
        self.last_time = self.last_time.max(r.timestamp);
        self.records += 1;
        if let Some(d) = r.distance_cm {
            self.distance_cm = self.distance_cm.max(d);
        }
        if let Some(v) = r.speed_mm_s {
            self.max_speed_mm_s = self.max_speed_mm_s.max(v);
        }
        if let Some(w) = r.power_w {
            self.power = (self.power.0 + w as u64, self.power.1 + 1);
        }
        if let Some(bpm) = r.heart_rate {
            self.heart_rate = (self.heart_rate.0 + bpm as u64, self.heart_rate.1 + 1);
        }
        if let Some(pa) = r.pressure_pa {
            let alt = altitude::pressure_altitude_dm(pa);
            let last = self.last_alt.replace((r.timestamp, alt));
            if let Some((base, since)) = self.held {
                if alt <= base + CLIMB_HYSTERESIS_DM || r.timestamp.saturating_sub(since) > SPIKE_HOLD_S {
                    self.held = None;
                } else {
                    return;
                }
            } else if let Some((t, a)) = last
                && let dt @ 1..=5 = r.timestamp.saturating_sub(t)
                && alt - a > MAX_CLIMB_DM_S * dt as i32
            {
                self.held = Some((a, r.timestamp));
                return;
            }
            let reference = self.climb_ref_dm.get_or_insert(alt);
            if alt >= *reference + CLIMB_HYSTERESIS_DM {
                self.ascent_dm += (alt - *reference) as u32;
                *reference = alt;
            } else if alt <= *reference - CLIMB_HYSTERESIS_DM {
                self.descent_dm += (*reference - alt) as u32;
                *reference = alt;
            }
        }
    }

    pub fn running(&self) -> bool {
        self.running_since.is_some()
    }

    /// Average power over the records that have one, watts.
    pub fn avg_power_w(&self) -> Option<u16> {
        let (sum, n) = self.power;
        (n > 0).then(|| ((sum + n as u64 / 2) / n as u64) as u16)
    }

    /// Average heart rate over the records that have one, bpm.
    pub fn avg_heart_rate(&self) -> Option<u8> {
        let (sum, n) = self.heart_rate;
        (n > 0).then(|| ((sum + n as u64 / 2) / n as u64) as u8)
    }

    pub fn totals(&self) -> Totals {
        let start = self.start_time.unwrap_or(0);
        let running = self
            .running_since
            .map_or(0, |since| self.last_time.saturating_sub(since));
        Totals {
            start_time: start,
            end_time: self.last_time,
            elapsed_ms: self.last_time.saturating_sub(start) * 1000,
            timer_ms: (self.timer_s + running) * 1000,
            distance_cm: self.distance_cm,
            ascent_m: (self.ascent_dm / 10).min(u16::MAX as u32) as u16,
            descent_m: (self.descent_dm / 10).min(u16::MAX as u32) as u16,
            max_speed_mm_s: self.max_speed_mm_s,
        }
    }
}

/// Below this GNSS speed the bike is taken as stopped: the receiver reports ~0.1–0.3 m/s of noise at rest.
pub const MOVING_MM_S: u32 = 500;
/// Longest gap between two speeds integrated as is (a lost fix); longer gaps count this long.
const MAX_GAP_S: u32 = 5;

/// Distance from the GNSS speed (until a wheel sensor gives a better one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Odometer {
    last: Option<u32>,
    distance_mm: u64,
}

impl Odometer {
    /// Goes on from `distance_cm` (a resumed ride).
    pub fn starting_at(distance_cm: u32) -> Self {
        Self {
            last: None,
            distance_mm: distance_cm as u64 * 10,
        }
    }

    /// Adds the speed at FIT timestamp `timestamp`; returns the distance since the start, centimetres.
    pub fn update(&mut self, timestamp: u32, speed_mm_s: Option<u32>) -> u32 {
        if let (Some(last), Some(v)) = (self.last, speed_mm_s)
            && v >= MOVING_MM_S
        {
            let dt = timestamp.saturating_sub(last).min(MAX_GAP_S);
            self.distance_mm += v as u64 * dt as u64;
        }
        if speed_mm_s.is_some() {
            self.last = Some(timestamp);
        }
        self.distance_cm()
    }

    pub fn distance_cm(&self) -> u32 {
        (self.distance_mm / 10) as u32
    }
}

/// Timestamps of the records, one per GNSS epoch, strictly increasing.
///
/// With a fix, the GNSS time. Without, the receiver still ticks once a second but its time can be off by tens of
/// seconds (REVERSE_NOTES.md), and the nRF RTC has a 1 s resolution with an unknown phase against the epochs: an
/// epoch can arrive before the RTC second changes. So the clock advances by the epoch's own step (its time of day
/// minus the previous one's) and only follows the RTC when the RTC is ahead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EpochClock {
    last_epoch: Option<u32>,
    last: Option<u32>,
}

impl EpochClock {
    /// `epoch_s`: the epoch's time of day in seconds (GNSS, valid or not); `gnss`: the FIT timestamp from a valid
    /// fix; `rtc`: the FIT timestamp from the nRF RTC. Returns the record timestamp, None to drop the epoch (no
    /// time at all, or it would not be after the previous record).
    pub fn next(&mut self, epoch_s: u32, gnss: Option<u32>, rtc: Option<u32>) -> Option<u32> {
        let step = self
            .last_epoch
            .map(|last| (epoch_s + 86_400 - last) % 86_400)
            .filter(|&step| step > 0 && step <= 60);
        self.last_epoch = Some(epoch_s);
        let stepped = self.last.zip(step).map(|(last, step)| last + step);
        let t = gnss.or_else(|| match (stepped, rtc) {
            (Some(s), Some(r)) => Some(s.max(r)),
            (s, r) => s.or(r),
        })?;
        if self.last.is_some_and(|last| t <= last) {
            return None;
        }
        self.last = Some(t);
        Some(t)
    }
}

/// A ride being recorded.
pub struct Recorder {
    id: u32,
    summary: Summary,
    buf: [u8; BUF],
    len: usize,
    last_flush: u32,
    /// One record every this many seconds, and the last one written.
    interval_s: u32,
    last_record: Option<u32>,
}

impl Recorder {
    /// Starts ride `id` (Unix start time, also the file name) at FIT timestamp `timestamp`. The file exists with
    /// its first messages when this returns `Ok`.
    pub fn start<S: Storage>(
        fs: &Filesystem<'_, S>,
        id: u32,
        timestamp: u32,
        serial_number: u32,
    ) -> Result<Self> {
        ride_store::start(fs, id)?;
        let mut r = Self {
            id,
            summary: Summary::default(),
            buf: [0; BUF],
            len: 0,
            last_flush: timestamp,
            interval_s: 1,
            last_record: None,
        };
        r.put_raw(&fit::header(0))?;
        let n = fit::encode_definitions(&mut r.buf[r.len..]).map_err(|_| Error::NO_SPACE)?;
        r.len += n;
        r.put(&FileId {
            serial_number,
            time_created: timestamp,
        })?;
        r.event(timestamp, EventType::Start)?;
        r.flush(fs)?;
        Ok(r)
    }

    /// Opens interrupted ride `id` again, paused, whatever the state it was left in: a timer that was running is
    /// stopped at the last message on flash (the time switched off does not count); closing messages written, or
    /// the file even sealed, are removed. Then [`Recorder::resume`], [`Recorder::record`] and [`Recorder::stop`] as
    /// for a new ride. Every step is one commit and leaves a ride that this, or [`recover`], takes again.
    pub fn reopen<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<Self> {
        let mut scan = scan(fs, id)?;
        if let Some(at) = scan.closing_at {
            // the closing messages and the seal go in one commit, with the placeholder header back
            ride_store::update(fs, id, |file| {
                file.set_len(at)?;
                file.seek(SeekFrom::Start(0))?;
                write_all(file, &fit::header(0))
            })?;
            scan = self::scan(fs, id)?;
        }
        let mut r = Self {
            id,
            summary: scan.summary,
            buf: [0; BUF],
            len: 0,
            last_flush: scan.summary.last_time,
            interval_s: 1,
            last_record: None,
        };
        let end = r.summary.last_time;
        r.pause(fs, end)?;
        Ok(r)
    }

    /// Opens finished ride `id` again, paused (the "resume" of the end screen): back to an unfinished ride, then
    /// [`Recorder::reopen`].
    pub fn reopen_finished<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<Self> {
        ride_store::unfinish(fs, id)?;
        Self::reopen(fs, id)
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// One record every `s` seconds from now on (1: every sample). The totals come from the records written: the
    /// distance and the timer stay exact (the distance is cumulative), the ascent, the maximum speed and the
    /// averages are sampled.
    pub fn set_interval(&mut self, s: u32) {
        self.interval_s = s.max(1);
    }

    pub fn interval_s(&self) -> u32 {
        self.interval_s
    }

    /// Adds a sample (only while the timer runs: paused samples are not recorded, and one every
    /// [`Recorder::set_interval`] seconds); appends to the file every [`FLUSH_EVERY_S`] seconds.
    pub fn record<S: Storage>(&mut self, fs: &Filesystem<'_, S>, r: &Record) -> Result<()> {
        if !self.summary.running()
            || self.last_record.is_some_and(|t| r.timestamp < t.saturating_add(self.interval_s))
        {
            return Ok(());
        }
        if self.len + Record::DEFINITION.data_len() > BUF {
            self.flush(fs)?;
        }
        self.put(r)?;
        self.last_record = Some(r.timestamp);
        if r.timestamp.saturating_sub(self.last_flush) >= FLUSH_EVERY_S {
            self.flush(fs)?;
            self.last_flush = r.timestamp;
        }
        Ok(())
    }

    /// Stops the timer (pause); written at once.
    pub fn pause<S: Storage>(&mut self, fs: &Filesystem<'_, S>, timestamp: u32) -> Result<()> {
        if self.summary.running() {
            self.event(timestamp, EventType::Stop)?;
            self.flush(fs)?;
        }
        Ok(())
    }

    pub fn resume<S: Storage>(&mut self, fs: &Filesystem<'_, S>, timestamp: u32) -> Result<()> {
        if !self.summary.running() {
            self.event(timestamp, EventType::Start)?;
            self.flush(fs)?;
        }
        Ok(())
    }

    /// Ends the ride: closing messages, then [`seal`]. `utc_offset_s` gives the activity's local time.
    pub fn stop<S: Storage>(
        mut self,
        fs: &Filesystem<'_, S>,
        timestamp: u32,
        utc_offset_s: i32,
    ) -> Result<Totals> {
        self.flush(fs)?;
        let end = timestamp.max(self.summary.last_time);
        let totals = close(fs, self.id, &mut self.summary, end, utc_offset_s)?;
        seal(fs, self.id)?;
        Ok(totals)
    }

    /// Writes what is buffered (committed when this returns `Ok`).
    pub fn flush<S: Storage>(&mut self, fs: &Filesystem<'_, S>) -> Result<()> {
        if self.len > 0 {
            ride_store::append(fs, self.id, &self.buf[..self.len])?;
            self.len = 0;
        }
        Ok(())
    }

    fn event(&mut self, timestamp: u32, kind: EventType) -> Result<()> {
        self.put(&Event { timestamp, kind })
    }

    /// Encodes `m` into the buffer and feeds it to the summary, as read back from its encoding.
    fn put<M: Message>(&mut self, m: &M) -> Result<()> {
        let n = m
            .encode(&mut self.buf[self.len..])
            .map_err(|_| Error::NO_SPACE)?;
        feed(&mut self.summary, &self.buf[self.len..self.len + n]);
        self.len += n;
        Ok(())
    }

    fn put_raw(&mut self, data: &[u8]) -> Result<()> {
        let end = self.len + data.len();
        self.buf
            .get_mut(self.len..end)
            .ok_or(Error::NO_SPACE)?
            .copy_from_slice(data);
        self.len = end;
        Ok(())
    }
}

fn feed(summary: &mut Summary, msg: &[u8]) {
    if let Some(r) = Record::decode(msg) {
        summary.record(&r);
    } else if let Some(e) = Event::decode(msg) {
        summary.event(&e);
    }
}

/// Appends the closing messages of ride `id` (one commit) and returns its totals.
fn close<S: Storage>(
    fs: &Filesystem<'_, S>,
    id: u32,
    summary: &mut Summary,
    end: u32,
    utc_offset_s: i32,
) -> Result<Totals> {
    summary.event(&Event {
        timestamp: end,
        kind: EventType::StopAll,
    });
    let totals = summary.totals();
    let mut buf = [0u8; 160];
    let mut n = 0;
    let event = Event {
        timestamp: end,
        kind: EventType::StopAll,
    };
    push(&mut buf, &mut n, &event)?;
    push(&mut buf, &mut n, &Lap(totals))?;
    let session = Session {
        totals,
        num_laps: 1,
    };
    push(&mut buf, &mut n, &session)?;
    let activity = Activity {
        timestamp: end,
        local_timestamp: end.wrapping_add_signed(utc_offset_s),
        timer_ms: totals.timer_ms,
    };
    push(&mut buf, &mut n, &activity)?;
    ride_store::append(fs, id, &buf[..n])?;
    Ok(totals)
}

fn push<M: Message>(buf: &mut [u8], n: &mut usize, m: &M) -> Result<()> {
    *n += m.encode(&mut buf[*n..]).map_err(|_| Error::NO_SPACE)?;
    Ok(())
}

/// Totals of finished ride `id` of `size` bytes, from its session message: the closing messages have fixed lengths,
/// only the end of the file is read.
pub fn finished_totals<S: Storage>(fs: &Filesystem<'_, S>, id: u32, size: usize) -> Result<Totals> {
    let session = Session::DEFINITION.data_len();
    let tail = session + Activity::DEFINITION.data_len() + 2;
    let mut buf = [0u8; 64];
    let at = size.checked_sub(tail).ok_or(Error::CORRUPTION)?;
    ride_store::read_at(fs, id, true, at, &mut buf[..tail])?;
    Session::decode(&buf[..session]).map(|s| s.totals).ok_or(Error::CORRUPTION)
}

/// Writes the real header and the CRC of ride `id` (one commit), then renames it to `.fit`. A sealed file is
/// recognised by a non-zero data size in its header.
pub fn seal<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    ride_store::update(fs, id, |file| {
        let len = file.len()?;
        let data_size = len.checked_sub(HEADER_LEN).ok_or(Error::CORRUPTION)? as u32;
        let header = fit::header(data_size);
        file.seek(SeekFrom::Start(0))?;
        write_all(file, &header)?;
        let mut crc = Crc::new().update(&header);
        file.seek(SeekFrom::Start(HEADER_LEN as u32))?;
        let mut chunk = [0u8; 256];
        loop {
            match file.read(&mut chunk)? {
                0 => break,
                n => crc = crc.update(&chunk[..n]),
            }
        }
        file.seek(SeekFrom::End(0))?;
        write_all(file, &crc.value().to_le_bytes())
    })?;
    ride_store::finish(fs, id)
}

fn write_all<S: Storage>(
    file: &ride_store::littlefs2::fs::File<'_, '_, S>,
    data: &[u8],
) -> Result<()> {
    let mut rest = data;
    while !rest.is_empty() {
        let n = file.write(rest)?;
        rest = &rest[n..];
    }
    Ok(())
}

/// What [`recover`] did with an interrupted ride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovered {
    /// Closed and sealed with the records that had been written.
    Closed { id: u32, totals: Totals },
    /// Already sealed before the cut: only renamed.
    Renamed { id: u32 },
    /// No record at all: deleted.
    Empty { id: u32 },
}

/// The newest interrupted ride (power cut, crash, switched off during a pause): the one to offer to resume with
/// [`Recorder::reopen`]; [`recover`] closes the others.
pub fn interrupted<S: Storage>(fs: &Filesystem<'_, S>) -> Result<Option<u32>> {
    let mut newest = None;
    ride_store::unfinished(fs, |id, _| newest = newest.max(Some(id)))?;
    Ok(newest)
}

/// The newest finished ride: the one the end screen and the menu offer to resume ([`Recorder::reopen_finished`]).
pub fn last_finished<S: Storage>(fs: &Filesystem<'_, S>) -> Result<Option<u32>> {
    let mut newest = None;
    ride_store::finished(fs, |id, _| newest = newest.max(Some(id)))?;
    Ok(newest)
}

/// Closes every ride whose recording was interrupted (power cut, crash) except `keep` (the one to resume), calling
/// `f` for each. The closing messages are stamped with the last timestamp found in the file.
pub fn recover<S: Storage>(
    fs: &Filesystem<'_, S>,
    utc_offset_s: i32,
    keep: Option<u32>,
    mut f: impl FnMut(Recovered),
) -> Result<()> {
    let mut ids = [0u32; 8];
    let mut count = 0;
    ride_store::unfinished(fs, |id, _| {
        if count < ids.len() {
            ids[count] = id;
            count += 1;
        }
    })?;
    for &id in ids[..count].iter().filter(|&&id| Some(id) != keep) {
        let mut head = [0u8; HEADER_LEN];
        let n = ride_store::read(fs, id, false, &mut head)?;
        if n == HEADER_LEN && head[4..8] != [0; 4] {
            ride_store::finish(fs, id)?;
            f(Recovered::Renamed { id });
            continue;
        }
        let Scan {
            mut summary,
            closing_at,
        } = scan(fs, id)?;
        let closed = closing_at.is_some();
        if summary.records == 0 {
            ride_store::remove_unfinished(fs, id)?;
            f(Recovered::Empty { id });
            continue;
        }
        let totals = if closed {
            summary.totals()
        } else {
            let end = summary.last_time;
            close(fs, id, &mut summary, end, utc_offset_s)?
        };
        seal(fs, id)?;
        f(Recovered::Closed { id, totals });
    }
    Ok(())
}

/// What [`scan`] read back from an unfinished ride.
struct Scan {
    summary: Summary,
    /// Offset of the closing messages (from the `stop_all` event), when they are there.
    closing_at: Option<usize>,
}

/// Reads the messages of unfinished ride `id` back into a summary (closing messages included), up to the data
/// size of its header when it is sealed (the CRC follows).
fn scan<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<Scan> {
    let mut summary = Summary::default();
    let mut closing_at = None;
    let mut head = [0u8; HEADER_LEN];
    let n = ride_store::read(fs, id, false, &mut head)?;
    let data_size = if n == HEADER_LEN {
        u32::from_le_bytes([head[4], head[5], head[6], head[7]])
    } else {
        0
    };
    let end = (data_size > 0).then(|| HEADER_LEN + data_size as usize);
    let mut buf = [0u8; 256];
    let mut offset = HEADER_LEN;
    loop {
        let want = end.map_or(buf.len(), |e| e.saturating_sub(offset).min(buf.len()));
        let n = ride_store::read_at(fs, id, false, offset, &mut buf[..want])?;
        // Appends are atomic and end on a message boundary: a message cut at the end of the file is corruption,
        // one cut at the end of a full window continues in the next read.
        let more = n == buf.len();
        let mut pos = 0;
        while pos < n {
            let avail = n - pos;
            let definition = buf[pos] & 0x40 != 0;
            if definition && avail < 6 {
                break;
            }
            let len = fit::record_len(buf[pos], definition.then(|| buf[pos + 5]))
                .ok_or(Error::CORRUPTION)?;
            if len > avail {
                break;
            }
            let msg = &buf[pos..pos + len];
            if closing_at.is_none()
                && Event::decode(msg).is_some_and(|e| e.kind == EventType::StopAll)
            {
                closing_at = Some(offset + pos);
            }
            feed(&mut summary, msg);
            pos += len;
        }
        if !more {
            return if pos == n {
                Ok(Scan {
                    summary,
                    closing_at,
                })
            } else {
                Err(Error::CORRUPTION)
            };
        }
        offset += pos;
    }
}
