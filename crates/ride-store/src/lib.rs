//! Ride storage on littlefs, hardware independent.
//!
//! A ride being recorded is `/rides/<id>.part`, grown by [`append`]: each call opens, writes and closes the file, and
//! littlefs commits on close, so every `append` that returned `Ok` is on flash, whatever happens next. [`finish`]
//! renames it to `/rides/<id>.fit` (atomic in littlefs). After a power cut, [`unfinished`] lists the rides to resume
//! or close. `<id>` is the ride start time (Unix seconds), like the vendor firmware's names. A ride the companion
//! app acknowledged (copied and checked) becomes `/rides/<id>.sent.fit` ([`mark_sent`]): the first to go when room
//! is needed.
//!
//! Nothing here formats on its own: a mount error is reported, never "fixed" by erasing the rides. Formatting is
//! [`littlefs2::fs::Filesystem::format`], called on an explicit request only.

#![no_std]

#[cfg(feature = "ram")]
extern crate alloc;

#[cfg(feature = "ram")]
pub mod ram;
pub mod room;

use core::fmt::Write as _;

pub use littlefs2;
use littlefs2::driver::Storage;
use littlefs2::fs::Filesystem;
use littlefs2::io::{Error, Result, SeekFrom};
use littlefs2::path;
use littlefs2::path::{Path, PathBuf};

pub const RIDES: &Path = path!("/rides");

const PART: &str = ".part";
const FIT: &str = ".fit";
const SENT: &str = ".sent.fit";

fn ride_path(id: u32, ext: &str) -> PathBuf {
    let mut buf = Name::default();
    write!(buf, "/rides/{id}{ext}").unwrap();
    PathBuf::try_from(buf.as_str()).unwrap()
}

/// Starts recording ride `id`: creates an empty `/rides/<id>.part` (truncated if it exists).
pub fn start<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    if !fs.exists(RIDES) {
        fs.create_dir(RIDES)?;
    }
    fs.create_file_and_then(&ride_path(id, PART), |_| Ok(()))
}

/// Appends to ride `id`; committed to flash when this returns `Ok`.
pub fn append<S: Storage>(fs: &Filesystem<'_, S>, id: u32, data: &[u8]) -> Result<()> {
    fs.open_file_with_options_and_then(
        |o| o.write(true).append(true),
        &ride_path(id, PART),
        |file| {
            let mut rest = data;
            while !rest.is_empty() {
                let n = file.write(rest)?;
                rest = &rest[n..];
            }
            Ok(())
        },
    )
}

/// Opens ride `id` (not finished) for reading and writing anywhere, e.g. to rewrite a header once the size is
/// known. Everything `f` writes is committed together when it returns `Ok` (littlefs commits on close): a power
/// cut before leaves the file as it was.
pub fn update<S: Storage, R>(
    fs: &Filesystem<'_, S>,
    id: u32,
    f: impl FnOnce(&littlefs2::fs::File<'_, '_, S>) -> Result<R>,
) -> Result<R> {
    fs.open_file_with_options_and_then(|o| o.read(true).write(true), &ride_path(id, PART), f)
}

/// `/rides/<id>.sent.fit` if the ride was acknowledged, else `/rides/<id>.fit` (finished rides).
fn finished_path<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> PathBuf {
    let sent = ride_path(id, SENT);
    if fs.exists(&sent) { sent } else { ride_path(id, FIT) }
}

/// Closes ride `id`: `/rides/<id>.part` becomes `/rides/<id>.fit`.
pub fn finish<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    fs.rename(&ride_path(id, PART), &ride_path(id, FIT))
}

/// Opens finished ride `id` again: `/rides/<id>.fit` becomes `/rides/<id>.part` (atomic), to resume it. It is no
/// longer acknowledged: what the app has is not the whole ride any more.
pub fn unfinish<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    fs.rename(&finished_path(fs, id), &ride_path(id, PART))
}

/// Marks finished ride `id` as acknowledged by the companion app (atomic, done again harmlessly).
pub fn mark_sent<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    if fs.exists(&ride_path(id, SENT)) {
        return Ok(());
    }
    fs.rename(&ride_path(id, FIT), &ride_path(id, SENT))
}

/// Deletes finished ride `id`, acknowledged or not.
pub fn remove<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    fs.remove(&finished_path(fs, id))
}

/// Deletes unfinished ride `id` (e.g. an empty one found after a power cut).
pub fn remove_unfinished<S: Storage>(fs: &Filesystem<'_, S>, id: u32) -> Result<()> {
    fs.remove(&ride_path(id, PART))
}

/// Calls `f(id, size)` for every finished ride.
pub fn finished<S: Storage>(fs: &Filesystem<'_, S>, mut f: impl FnMut(u32, usize)) -> Result<()> {
    finished_sent(fs, |id, size, _| f(id, size))
}

/// Calls `f(id, size, sent)` for every finished ride, `sent` once the companion app acknowledged it.
pub fn finished_sent<S: Storage>(fs: &Filesystem<'_, S>, mut f: impl FnMut(u32, usize, bool)) -> Result<()> {
    rides(fs, |name, size| {
        if let Some(id) = name.strip_suffix(SENT).and_then(|id| id.parse().ok()) {
            f(id, size, true);
        } else if let Some(id) = name.strip_suffix(FIT).and_then(|id| id.parse().ok()) {
            f(id, size, false);
        }
    })
}

/// Calls `f(id, size)` for every ride whose recording was interrupted (power cut, crash).
pub fn unfinished<S: Storage>(fs: &Filesystem<'_, S>, mut f: impl FnMut(u32, usize)) -> Result<()> {
    rides(fs, |name, size| {
        if let Some(id) = name.strip_suffix(PART).and_then(|id| id.parse().ok()) {
            f(id, size);
        }
    })
}

fn rides<S: Storage>(fs: &Filesystem<'_, S>, mut f: impl FnMut(&str, usize)) -> Result<()> {
    if !fs.exists(RIDES) {
        return Ok(());
    }
    fs.read_dir_and_then(RIDES, |dir| {
        for entry in dir {
            let entry = entry?;
            f(entry.file_name().as_str(), entry.metadata().len());
        }
        Ok(())
    })
}

/// Reads ride `id` (finished or not) into `buf`, returns the length read.
pub fn read<S: Storage>(
    fs: &Filesystem<'_, S>,
    id: u32,
    finished: bool,
    buf: &mut [u8],
) -> Result<usize> {
    read_at(fs, id, finished, 0, buf)
}

/// Reads ride `id` (finished or not) from byte `offset` into `buf`, returns the length read.
pub fn read_at<S: Storage>(
    fs: &Filesystem<'_, S>,
    id: u32,
    finished: bool,
    offset: usize,
    buf: &mut [u8],
) -> Result<usize> {
    let path = if finished { finished_path(fs, id) } else { ride_path(id, PART) };
    fs.open_file_and_then(&path, |file| {
        file.seek(SeekFrom::Start(offset as u32))?;
        let mut n = 0;
        while n < buf.len() {
            match file.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        Ok(n)
    })
}

/// Fixed-size buffer for path names (no allocation).
#[derive(Default)]
struct Name {
    buf: [u8; 32],
    len: usize,
}

impl Name {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap()
    }
}

impl core::fmt::Write for Name {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        self.buf
            .get_mut(self.len..end)
            .ok_or(core::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Error returned by the fake flash once the power has been cut.
pub const POWER_CUT: Error = Error::IO;
