//! Hardware-in-the-loop power-cut test of the ride store (feature `hil-storage`, never in a normal build).
//!
//! The firmware records a fake ride without end, logging every append that returned Ok (`hil: acked <id> <len>`).
//! tools/hil_storage.py resets the ESP32 at random moments; at each boot [`recover`] checks every ride byte for byte
//! against the known pattern and logs what it found (`hil: ride <id> <state> <len> ok|BAD`), the host compares it with
//! the last acked length before the reset.

use littlefs2::driver::Storage;
use littlefs2::fs::Filesystem;
use littlefs2::io::Result;
use log::{info, warn};

const CHUNK: usize = 512;
/// A ride is closed (renamed) after this many bytes, to exercise `finish` too.
const RIDE_LEN: usize = 40 * CHUNK;
/// Finished rides kept; older ones are removed, to exercise `remove` and keep the partition from filling up.
const KEEP: usize = 3;

/// Byte `i` of ride `id`.
fn pattern(id: u32, i: usize) -> u8 {
    ((i as u32).wrapping_mul(2_654_435_761) ^ id.wrapping_mul(40_503)).rotate_left(8) as u8
}

/// Offset of the first byte of ride `id` that differs from the pattern, if any.
fn first_bad<S: Storage>(
    fs: &Filesystem<'_, S>,
    id: u32,
    finished: bool,
    len: usize,
) -> Result<Option<usize>> {
    let mut buf = [0u8; CHUNK];
    let mut off = 0;
    while off < len {
        let n = ride_store::read_at(fs, id, finished, off, &mut buf[..CHUNK.min(len - off)])?;
        if let Some(k) = (0..n).find(|&k| buf[k] != pattern(id, off + k)) {
            return Ok(Some(off + k));
        }
        if n == 0 {
            return Ok(Some(off));
        }
        off += n;
    }
    Ok(None)
}

/// Checks and closes what the previous boot left; returns the id to record next.
pub fn recover<S: Storage>(fs: &Filesystem<'_, S>) -> Result<u32> {
    let (mut unfinished, mut finished) = (ride_ids(), ride_ids());
    ride_store::unfinished(fs, |id, len| unfinished.push((id, len)))?;
    ride_store::finished(fs, |id, len| finished.push((id, len)))?;
    for &(id, len) in &unfinished {
        log_check("unfinished", id, len, first_bad(fs, id, false, len)?);
        ride_store::finish(fs, id)?;
        finished.push((id, len));
    }
    finished.sort_unstable();
    for &(id, len) in &finished {
        log_check("finished", id, len, first_bad(fs, id, true, len)?);
    }
    let excess = finished.len().saturating_sub(KEEP);
    for &(id, _) in &finished[..excess] {
        ride_store::remove(fs, id)?;
    }
    let next = finished.last().map_or(1, |&(id, _)| id + 1);
    info!(
        "hil: boot, {} recovered, {} finished, next ride {next}",
        unfinished.len(),
        finished.len()
    );
    Ok(next)
}

fn log_check(state: &str, id: u32, len: usize, bad: Option<usize>) {
    match bad {
        None => info!("hil: ride {id} {state} {len} ok"),
        Some(at) => warn!("hil: ride {id} {state} {len} BAD at {at}"),
    }
}

fn ride_ids() -> alloc::vec::Vec<(u32, usize)> {
    alloc::vec::Vec::new()
}

/// The ride being recorded.
pub struct Recorder {
    id: u32,
    len: usize,
}

impl Recorder {
    pub fn start<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<Self> {
        ride_store::start(fs, id)?;
        info!("hil: acked {id} 0");
        Ok(Self { id, len: 0 })
    }

    /// One append; closes the ride and starts the next one when it is long enough.
    pub fn step<S: Storage>(&mut self, fs: &Filesystem<'_, S>) -> Result<()> {
        let mut chunk = [0u8; CHUNK];
        for (k, b) in chunk.iter_mut().enumerate() {
            *b = pattern(self.id, self.len + k);
        }
        ride_store::append(fs, self.id, &chunk)?;
        self.len += CHUNK;
        info!("hil: acked {} {}", self.id, self.len);
        if self.len >= RIDE_LEN {
            ride_store::finish(fs, self.id)?;
            info!("hil: finished {} {}", self.id, self.len);
            *self = Self::start(fs, self.id + 1)?;
        }
        Ok(())
    }
}
