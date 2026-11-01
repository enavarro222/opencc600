//! One permanent atlas, independent of the route and corridor files.
#![no_std]
extern crate alloc;

use alloc::{vec, vec::Vec};
use littlefs2::{
    driver::Storage,
    fs::Filesystem,
    io::{Error, Result},
    path,
};

pub const MAX_BYTES: usize = route::atlas::MAX_BYTES;
pub const FILE: &littlefs2::path::Path = path!("/atlas");
const NEW: &littlefs2::path::Path = path!("/atlas.new");

pub fn load<S: Storage>(fs: &Filesystem<'_, S>) -> Result<Option<Vec<u8>>> {
    // A temporary file from an interrupted update is never the installed atlas.
    if fs.exists(NEW) {
        fs.remove(NEW)?;
    }
    if !fs.exists(FILE) {
        return Ok(None);
    }
    let size = fs.metadata(FILE)?.len();
    if size > MAX_BYTES {
        return Err(Error::CORRUPTION);
    }
    let mut data = vec![0; size];
    fs.open_file_and_then(FILE, |file| {
        let mut at = 0;
        while at < size {
            let n = file.read(&mut data[at..])?;
            if n == 0 {
                return Err(Error::CORRUPTION);
            }
            at += n;
        }
        Ok(())
    })?;
    route::atlas::Atlas::parse(&data).map_err(|_| Error::CORRUPTION)?;
    Ok(Some(data))
}

/// Validate before any write, then rename after the temporary file is closed.
/// A power cut leaves either the old atlas or the complete new one.
pub fn store<S: Storage>(fs: &Filesystem<'_, S>, data: &[u8]) -> Result<()> {
    route::atlas::Atlas::parse(data).map_err(|_| Error::CORRUPTION)?;
    fs.write(NEW, data)?;
    fs.rename(NEW, FILE)
}
