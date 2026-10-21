//! Room on the storage: the rides first.
//!
//! The technical logs (`/logs/<start unix>.csv`, the trip log) give way to the rides: together they never take more
//! than [`LOGS_MAX_BYTES`], they never write into the last [`RESERVE_BYTES`], and a ride short of room deletes them,
//! the oldest first ([`for_ride`]). A full littlefs is slow (every allocation walks the whole file system) and wears
//! the few blocks left: the reserve keeps the rides away from it too.

use core::fmt::Write as _;

use littlefs2::driver::Storage;
use littlefs2::fs::Filesystem;
use littlefs2::io::Result;
use littlefs2::path;
use littlefs2::path::{Path, PathBuf};

use crate::Name;

pub const LOGS: &Path = path!("/logs");

/// Kept for the rides: the logs never write into it (~8 h of ride, ~40 h with a record every 5 s).
pub const RESERVE_BYTES: usize = 1 << 20;
/// With less room than this for the rides (the logs' included), a ride records one point every 5 s
/// (`recorder::SPARSE_INTERVAL_S`).
pub const SPARSE_BYTES: usize = RESERVE_BYTES;
/// The logs all together, at most (~3 h of trip log).
pub const LOGS_MAX_BYTES: usize = 2 << 20;
/// A log is cut into files of about this size, so that the oldest part of a long session goes first.
pub const LOG_FILE_BYTES: usize = 512 << 10;
/// A ride does not start with less free (~2.5 h with a record every 5 s): it would stop recording soon.
pub const RIDE_START_BYTES: usize = 64 << 10;

/// `/logs/<id>.csv`.
pub fn log_path(id: u32) -> PathBuf {
    let mut buf = Name::default();
    write!(buf, "/logs/{id}.csv").unwrap();
    PathBuf::try_from(buf.as_str()).unwrap()
}

/// Calls `f(id, size)` for every log.
pub fn logs<S: Storage>(fs: &Filesystem<'_, S>, mut f: impl FnMut(u32, usize)) -> Result<()> {
    if !fs.exists(LOGS) {
        return Ok(());
    }
    fs.read_dir_and_then(LOGS, |dir| {
        for entry in dir {
            let entry = entry?;
            if let Some(id) = entry.file_name().as_str().strip_suffix(".csv").and_then(|id| id.parse().ok()) {
                f(id, entry.metadata().len());
            }
        }
        Ok(())
    })
}

/// Size of all the logs, bytes.
pub fn logs_bytes<S: Storage>(fs: &Filesystem<'_, S>) -> Result<usize> {
    let mut total = 0;
    logs(fs, |_, len| total += len)?;
    Ok(total)
}

/// Free bytes (a walk through the whole file system).
pub fn free_bytes<S: Storage>(fs: &Filesystem<'_, S>) -> Result<usize> {
    Ok(fs.available_blocks()? * S::BLOCK_SIZE)
}

/// Deletes the oldest logs until the logs take at most `logs_max` bytes and `free_min` bytes are free, or no log is
/// left. Returns the free bytes.
pub fn trim_logs<S: Storage>(fs: &Filesystem<'_, S>, logs_max: usize, free_min: usize) -> Result<usize> {
    loop {
        let free = free_bytes(fs)?;
        let mut oldest: Option<u32> = None;
        let mut total = 0;
        logs(fs, |id, len| {
            total += len;
            oldest = Some(oldest.map_or(id, |o| o.min(id)));
        })?;
        match oldest {
            Some(id) if total > logs_max || free < free_min => fs.remove(&log_path(id))?,
            _ => return Ok(free),
        }
    }
}

/// Makes room for a new log file of [`LOG_FILE_BYTES`] (older logs deleted): whether it may be written.
pub fn for_log<S: Storage>(fs: &Filesystem<'_, S>) -> Result<bool> {
    let free_min = RESERVE_BYTES + LOG_FILE_BYTES;
    Ok(trim_logs(fs, LOGS_MAX_BYTES - LOG_FILE_BYTES, free_min)? >= free_min)
}

/// Makes room for a ride: logs deleted until the reserve is free, all of them if needed. Returns the free bytes.
pub fn for_ride<S: Storage>(fs: &Filesystem<'_, S>) -> Result<usize> {
    trim_logs(fs, usize::MAX, RESERVE_BYTES)
}
