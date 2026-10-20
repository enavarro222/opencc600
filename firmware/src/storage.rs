//! The `storage` flash partition as a littlefs block device.
//!
//! The partition is ours (docs/PRODUCT.md, storage decision): the vendor's content is backed up in dump/ and
//! goes back with `tools/flash_ota0.py --restore-vendor --storage`. [`open`] formats only a blank (erased)
//! partition, never one that fails to mount for another reason.

use alloc::boxed::Box;

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::FlashStorage;
use littlefs2::consts::{U4, U256};
use littlefs2::driver::Storage;
use littlefs2::fs::{Allocation, Filesystem};
use littlefs2::io::{Error, Result};

/// `storage` partition (data/0x83), checked against the device partition table by tools/flash_ota0.py.
pub const OFFSET: u32 = 0x86_0000;
pub const SIZE: usize = 0x79_E000;
pub const BLOCK: usize = 4096;

pub struct Partition<'d> {
    flash: FlashStorage<'d>,
}

impl<'d> Partition<'d> {
    pub fn new(flash: FlashStorage<'d>) -> Self {
        Self { flash }
    }

    /// Both superblock blocks fully erased: nothing to lose by formatting.
    fn is_blank(&mut self) -> Result<bool> {
        let mut buf = [0u8; 256];
        for off in (0..2 * BLOCK).step_by(buf.len()) {
            self.read(off, &mut buf)?;
            if buf.iter().any(|&b| b != 0xFF) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Mounts the partition; formats it first if (and only if) it is blank. Returns whether it was formatted.
pub fn open<'a, 'd>(
    alloc: &'a mut Box<Allocation<Partition<'d>>>,
    partition: &'a mut Partition<'d>,
) -> Result<(Filesystem<'a, Partition<'d>>, bool)> {
    let formatted = if Filesystem::is_mountable(partition) {
        false
    } else if partition.is_blank()? {
        Filesystem::format(partition)?;
        true
    } else {
        return Err(Error::CORRUPTION);
    };
    Ok((Filesystem::mount(alloc, partition)?, formatted))
}

impl Storage for Partition<'_> {
    /// 4 KiB blocks = flash sectors (the vendor used the same geometry).
    const BLOCK_SIZE: usize = BLOCK;
    const BLOCK_COUNT: usize = SIZE / BLOCK;
    const READ_SIZE: usize = 16;
    const WRITE_SIZE: usize = 16;
    type CACHE_SIZE = U256;
    /// In u64 words: 4 × 64 = 256 blocks scanned per allocation pass.
    type LOOKAHEAD_SIZE = U4;
    /// Wear levelling: move a metadata pair after this many erases (littlefs recommends 100..1000).
    const BLOCK_CYCLES: isize = 500;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> Result<usize> {
        ReadNorFlash::read(&mut self.flash, OFFSET + off as u32, buf).map_err(|_| Error::IO)?;
        Ok(buf.len())
    }

    fn write(&mut self, off: usize, data: &[u8]) -> Result<usize> {
        NorFlash::write(&mut self.flash, OFFSET + off as u32, data).map_err(|_| Error::IO)?;
        Ok(data.len())
    }

    fn erase(&mut self, off: usize, len: usize) -> Result<usize> {
        let from = OFFSET + off as u32;
        NorFlash::erase(&mut self.flash, from, from + len as u32).map_err(|_| Error::IO)?;
        Ok(len)
    }
}

// C library functions littlefs uses to split paths, not provided by littlefs2's `c-stubs` (no libc here).

/// # Safety
/// `s` and `accept` must be NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strspn(s: *const u8, accept: *const u8) -> usize {
    // SAFETY: both are NUL-terminated (caller contract).
    unsafe { span(s, accept, true) }
}

/// # Safety
/// `s` and `reject` must be NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    // SAFETY: both are NUL-terminated (caller contract).
    unsafe { span(s, reject, false) }
}

/// Length of the prefix of `s` whose bytes are all in `set` (`inside`) or all out of it (`!inside`).
unsafe fn span(s: *const u8, set: *const u8, inside: bool) -> usize {
    // SAFETY: NUL-terminated strings, read up to their terminator only.
    let set = unsafe { core::ffi::CStr::from_ptr(set.cast()) }.to_bytes();
    let s = unsafe { core::ffi::CStr::from_ptr(s.cast()) }.to_bytes();
    s.iter().take_while(|b| set.contains(b) == inside).count()
}
