//! The `storage` flash partition as a littlefs block device.
//!
//! Read-only for now: write and erase fail, so mounting the partition cannot change it whatever littlefs does.
//! Writes come with the move to our own layout (docs/PRODUCT.md, storage decision).

use esp_storage::FlashStorage;
use littlefs2::consts::{U4, U256};
use littlefs2::driver::Storage;
use littlefs2::io::{Error, Result};

/// `storage` partition (data/0x83), checked against the device partition table by tools/flash_ota0.py.
pub const OFFSET: u32 = 0x86_0000;
pub const SIZE: usize = 0x79_E000;

pub struct Partition<'d> {
    flash: FlashStorage<'d>,
}

impl<'d> Partition<'d> {
    pub fn read_only(flash: FlashStorage<'d>) -> Self {
        Self { flash }
    }
}

impl Storage for Partition<'_> {
    /// Same geometry as the vendor's (littlefs superblock): 4 KiB blocks = flash sectors.
    const BLOCK_SIZE: usize = 4096;
    const BLOCK_COUNT: usize = SIZE / 4096;
    const READ_SIZE: usize = 16;
    const WRITE_SIZE: usize = 16;
    type CACHE_SIZE = U256;
    /// In u64 words: 4 × 64 = 256 blocks scanned per allocation pass.
    type LOOKAHEAD_SIZE = U4;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> Result<usize> {
        self.flash
            .read(OFFSET + off as u32, buf)
            .map_err(|_| Error::IO)?;
        Ok(buf.len())
    }

    fn write(&mut self, _off: usize, _data: &[u8]) -> Result<usize> {
        Err(Error::IO)
    }

    fn erase(&mut self, _off: usize, _len: usize) -> Result<usize> {
        Err(Error::IO)
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
