//! Background map of a route, read in place on the device: OSM roads, tracks, paths, water and forests around the
//! route, prepared off the device ([`crate::build_map`]) on the route's plane, in metres.
//!
//! Two levels: the detail, for the zooms around the rider, and the overview of the whole route, simplified for its
//! scale. Each level is cut in square cells of the plane, listed in an index sorted by row then column: the device
//! reads only the cells in view.
//!
//! File (little endian): `OCMP`, version 1, the CRC-16 of the route file it belongs to, the level count, for each
//! level its cell size (m), cell count and index offset; each index is the cells' (column, row, offset), 8 bytes
//! each, then the end of the last one. A cell is a list of features: class, point count, byte count, then the points
//! as deltas (metres, zigzag varints) from the cell's corner. Areas come first. Then a CRC-16 of all that.

use crate::{Error, Reader, crc16};

pub const MAGIC: &[u8; 4] = b"OCMP";
pub const VERSION: u8 = 1;
/// Points of a feature at most (a piece of a cell): the device projects them into a fixed buffer.
pub const MAX_POINTS: usize = 255;
pub const MAX_LEVELS: usize = 2;
/// The level of the zooms around the rider, and the one of the whole route.
pub const DETAIL: usize = 0;
pub const OVERVIEW: usize = 1;

/// Feature classes, in drawing order: the areas, then the lines from the thinnest.
pub mod class {
    pub const FOREST: u8 = 0;
    pub const WATER: u8 = 1;
    pub const RESIDENTIAL: u8 = 2;
    /// Contour lines, and the major ones (every 50 m).
    pub const CONTOUR: u8 = 3;
    pub const CONTOUR_MAJOR: u8 = 4;
    pub const STREAM: u8 = 5;
    pub const RIVER: u8 = 6;
    pub const PATH: u8 = 7;
    pub const TRACK: u8 = 8;
    pub const SERVICE: u8 = 9;
    pub const MINOR: u8 = 10;
    pub const MAJOR: u8 = 11;
    pub const COUNT: usize = 12;

    /// Filled areas (closed rings); the others are lines.
    pub const fn is_area(c: u8) -> bool {
        c <= RESIDENTIAL
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub cell_m: u32,
    pub cells: u32,
    index_at: usize,
}

/// A map file, read in place.
#[derive(Clone, Copy, Debug)]
pub struct Map<'a> {
    data: &'a [u8],
    /// CRC-16 of the route file the map was made for.
    pub route_crc: u16,
    levels: [Option<Level>; MAX_LEVELS],
}

impl<'a> Map<'a> {
    /// Checks the file (magic, version, CRC, index) and reads its header.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < 8 || &data[..4] != MAGIC || data[4] != VERSION {
            return Err(Error::Format);
        }
        let (body, crc) = data.split_at(data.len() - 2);
        if crc16(body).to_le_bytes() != crc {
            return Err(Error::Corrupt);
        }
        let mut r = Reader { data: body, at: 5 };
        let route_crc = r.u16()?;
        let n = r.u8()? as usize;
        if n == 0 || n > MAX_LEVELS {
            return Err(Error::Format);
        }
        let mut levels = [None; MAX_LEVELS];
        for level in levels.iter_mut().take(n) {
            let l = Level {
                cell_m: r.u16()? as u32,
                cells: r.u32()?,
                index_at: r.u32()? as usize,
            };
            // the index and every cell's data are in the file, in order
            let end = l.index_at + l.cells as usize * 8 + 4;
            if l.cell_m == 0 || end > body.len() {
                return Err(Error::Corrupt);
            }
            let mut last = None;
            for i in 0..=l.cells as usize {
                let at = start(body, l, i);
                if at > body.len() || last.is_some_and(|p| at < p) {
                    return Err(Error::Corrupt);
                }
                last = Some(at);
            }
            *level = Some(l);
        }
        Ok(Self { data: body, route_crc, levels })
    }

    pub fn level(&self, i: usize) -> Option<Level> {
        self.levels.get(i).copied().flatten()
    }

    /// The cells of a level in columns `cx0..=cx1`, rows `cy0..=cy1`.
    pub fn cells(&self, level: Level, (cx0, cy0): (i32, i32), (cx1, cy1): (i32, i32)) -> impl Iterator<Item = Cell<'a>> + '_ {
        (cy0..=cy1).flat_map(move |cy| {
            // the first cell of the row from cx0, by bisection
            let (mut lo, mut hi) = (0usize, level.cells as usize);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let (x, y) = self.key(level, mid);
                if (y as i32, x as i32) < (cy, cx0) {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            (lo..level.cells as usize)
                .map_while(move |i| {
                    let (x, y) = self.key(level, i);
                    (y as i32 == cy && x as i32 <= cx1).then(|| self.cell(level, i))
                })
        })
    }

    /// The first and last rows of a level (all its cells: [`Map::cells`] over them and every column).
    pub fn rows(&self, level: Level) -> Option<(i32, i32)> {
        (level.cells > 0).then(|| (self.key(level, 0).1 as i32, self.key(level, level.cells as usize - 1).1 as i32))
    }

    fn key(&self, level: Level, i: usize) -> (i16, i16) {
        let at = level.index_at + i * 8;
        (
            i16::from_le_bytes([self.data[at], self.data[at + 1]]),
            i16::from_le_bytes([self.data[at + 2], self.data[at + 3]]),
        )
    }

    fn cell(&self, level: Level, i: usize) -> Cell<'a> {
        let (cx, cy) = self.key(level, i);
        let (from, to) = (start(self.data, level, i), start(self.data, level, i + 1));
        Cell {
            origin_m: (cx as i32 * level.cell_m as i32, cy as i32 * level.cell_m as i32),
            data: &self.data[from..to],
        }
    }
}

/// Where cell `i` of a level starts; `i == cells`: where the last one ends.
fn start(data: &[u8], level: Level, i: usize) -> usize {
    let at = if i == level.cells as usize { level.index_at + i * 8 } else { level.index_at + i * 8 + 4 };
    u32_at(data, at) as usize
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// A cell of a level: its corner on the route's plane (metres) and its features.
#[derive(Clone, Copy, Debug)]
pub struct Cell<'a> {
    pub origin_m: (i32, i32),
    data: &'a [u8],
}

impl<'a> Cell<'a> {
    pub fn features(&self) -> Features<'a> {
        Features {
            r: Reader { data: self.data, at: 0 },
            origin_m: self.origin_m,
        }
    }
}

pub struct Features<'a> {
    r: Reader<'a>,
    origin_m: (i32, i32),
}

/// A line or an area ring: its class and points.
#[derive(Clone, Copy, Debug)]
pub struct Feature<'a> {
    pub class: u8,
    pub count: u8,
    data: &'a [u8],
    origin_m: (i32, i32),
}

impl<'a> Feature<'a> {
    /// The points on the route's plane, metres.
    pub fn points(&self) -> impl Iterator<Item = (i32, i32)> + 'a {
        let mut r = Reader { data: self.data, at: 0 };
        let mut last = self.origin_m;
        (0..self.count).map_while(move |_| {
            last = (last.0.wrapping_add(r.zigzag().ok()?), last.1.wrapping_add(r.zigzag().ok()?));
            Some(last)
        })
    }
}

impl<'a> Iterator for Features<'a> {
    type Item = Feature<'a>;

    fn next(&mut self) -> Option<Feature<'a>> {
        if self.r.at >= self.r.data.len() {
            return None;
        }
        let class = self.r.u8().ok()?;
        let count = self.r.varint().ok()?.min(MAX_POINTS as u32) as u8;
        let len = self.r.varint().ok()? as usize;
        let data = self.r.data.get(self.r.at..self.r.at + len)?;
        self.r.at += len;
        Some(Feature { class, count, data, origin_m: self.origin_m })
    }
}
