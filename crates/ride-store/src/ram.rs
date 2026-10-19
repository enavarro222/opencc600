//! RAM-backed NOR flash with power-cut injection.
//!
//! NOR semantics like the ESP32 flash: erase sets a block to 0xFF, programming can only clear bits (new = old & data).
//! [`RamFlash::cut_after`] makes the n-th program or erase from now be interrupted: only its first half is applied,
//! it returns an error, and every later access fails until [`RamFlash::power_on`], as a real power cut would.

use alloc::vec;
use alloc::vec::Vec;

use littlefs2::consts::{U4, U256};
use littlefs2::driver::Storage;
use littlefs2::io::Result;

use crate::POWER_CUT;

pub const BLOCK_SIZE: usize = 4096;

#[derive(Clone)]
pub struct RamFlash<const BLOCKS: usize> {
    pub data: Vec<u8>,
    /// Programs + erases since creation.
    pub ops: u32,
    cut_at: Option<u32>,
    off: bool,
}

impl<const BLOCKS: usize> Default for RamFlash<BLOCKS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const BLOCKS: usize> RamFlash<BLOCKS> {
    /// Erased flash.
    pub fn new() -> Self {
        Self {
            data: vec![0xFF; BLOCKS * BLOCK_SIZE],
            ops: 0,
            cut_at: None,
            off: false,
        }
    }

    /// Cuts the power during the `n`-th program or erase from now (n >= 1).
    pub fn cut_after(&mut self, n: u32) {
        self.cut_at = Some(self.ops + n);
    }

    /// Whether the power has been cut.
    pub fn is_off(&self) -> bool {
        self.off
    }

    /// Power back on: the content stays as the cut left it.
    pub fn power_on(&mut self) {
        self.off = false;
        self.cut_at = None;
    }

    /// Counts an operation; true if it is the one interrupted by the cut.
    fn op(&mut self) -> Result<bool> {
        if self.off {
            return Err(POWER_CUT);
        }
        self.ops += 1;
        Ok(self.cut_at == Some(self.ops))
    }
}

impl<const BLOCKS: usize> Storage for RamFlash<BLOCKS> {
    // Same geometry as the CC600 partition (firmware `storage::Partition`), fewer blocks.
    const BLOCK_SIZE: usize = BLOCK_SIZE;
    const BLOCK_COUNT: usize = BLOCKS;
    const READ_SIZE: usize = 16;
    const WRITE_SIZE: usize = 16;
    type CACHE_SIZE = U256;
    type LOOKAHEAD_SIZE = U4;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> Result<usize> {
        if self.off {
            return Err(POWER_CUT);
        }
        buf.copy_from_slice(&self.data[off..off + buf.len()]);
        Ok(buf.len())
    }

    fn write(&mut self, off: usize, data: &[u8]) -> Result<usize> {
        let cut = self.op()?;
        let n = if cut { data.len() / 2 } else { data.len() };
        for (dst, src) in self.data[off..off + n].iter_mut().zip(data) {
            *dst &= *src;
        }
        if cut {
            self.off = true;
            return Err(POWER_CUT);
        }
        Ok(data.len())
    }

    fn erase(&mut self, off: usize, len: usize) -> Result<usize> {
        let cut = self.op()?;
        let n = if cut { len / 2 } else { len };
        self.data[off..off + n].fill(0xFF);
        if cut {
            self.off = true;
            return Err(POWER_CUT);
        }
        Ok(len)
    }
}
