//! Route file of the CC600, hardware independent. A route is prepared off the device ([`build`], feature `build`:
//! the PC tool, the browser, later the app): read from a GPX, simplified, projected on a local plane in decimetres,
//! written compact. The device reads it in place with integers only ([`Route`]): the points, where the rider is
//! along it ([`Route::locate`]) and how far from it.
//!
//! File (little endian): `OCRT`, version 1, the name, the projection (origin, decimetres per 1e-7 degree of
//! longitude and latitude, ×2^24), the point count, length, ascent, descent, elevation range, bounding box, then
//! for each point the deltas from the previous one (x, y, elevation, segment length), zigzag varints, then a CRC-16.
//! The projection is integer on both sides, so the device places the rider exactly as the points were placed.

#![no_std]

#[cfg(feature = "build")]
extern crate alloc;

#[cfg(feature = "build")]
mod build;
#[cfg(feature = "build")]
mod build_map;
pub mod map;
#[cfg(feature = "build")]
pub use build::{Gpx, GpxError, build, parse_gpx};
#[cfg(feature = "build")]
pub use build_map::{MapFeature, MapStats, build_map};

pub const MAGIC: &[u8; 4] = b"OCRT";
pub const VERSION: u8 = 1;
/// Longest name kept, bytes.
pub const NAME_MAX: usize = 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a route file, or another version.
    Format,
    /// Cut or damaged.
    Corrupt,
}

/// The header of a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub lat0_e7: i32,
    pub lon0_e7: i32,
    kx_q24: i32,
    ky_q24: i32,
    pub count: u32,
    /// Length along the points, decimetres.
    pub length_dm: u32,
    pub ascent_m: u16,
    pub descent_m: u16,
    pub min_ele_dm: i32,
    pub max_ele_dm: i32,
    /// Bounding box, decimetres: min x, min y, max x, max y.
    pub bbox: [i32; 4],
}

/// A point of the route: position on the local plane (x east, y north, decimetres), elevation, distance from the
/// start along the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
    pub ele_dm: i32,
    pub along_dm: u32,
}

/// Where the rider is relative to the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Distance from the start along the route, decimetres.
    pub along_dm: u32,
    /// Distance from the route, decimetres.
    pub off_dm: u32,
    /// The segment the rider is on (from point `segment` to the next one).
    pub segment: u32,
}

/// A route file, read in place.
#[derive(Clone, Copy, Debug)]
pub struct Route<'a> {
    data: &'a [u8],
    name: (usize, usize),
    points_at: usize,
    pub info: Info,
}

impl<'a> Route<'a> {
    /// Checks the file (magic, version, CRC, every point) and reads its header.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < 6 || &data[..4] != MAGIC || data[4] != VERSION {
            return Err(Error::Format);
        }
        let (body, crc) = data.split_at(data.len().checked_sub(2).ok_or(Error::Corrupt)?);
        if crc16(body).to_le_bytes() != crc {
            return Err(Error::Corrupt);
        }
        let mut r = Reader { data: body, at: 5 };
        let name_len = r.u8()? as usize;
        let name = (r.at, r.at + name_len);
        r.skip(name_len)?;
        core::str::from_utf8(body.get(name.0..name.1).ok_or(Error::Corrupt)?).map_err(|_| Error::Corrupt)?;
        let info = Info {
            lat0_e7: r.i32()?,
            lon0_e7: r.i32()?,
            kx_q24: r.i32()?,
            ky_q24: r.i32()?,
            count: r.u32()?,
            length_dm: r.u32()?,
            ascent_m: r.u16()?,
            descent_m: r.u16()?,
            min_ele_dm: r.i32()?,
            max_ele_dm: r.i32()?,
            bbox: [r.i32()?, r.i32()?, r.i32()?, r.i32()?],
        };
        let route = Self {
            data: body,
            name,
            points_at: r.at,
            info,
        };
        // every point decodes and the data ends with the last one
        let mut points = route.points();
        for _ in 0..info.count {
            points.next().ok_or(Error::Corrupt)?;
        }
        if points.r.at != body.len() || info.count < 2 {
            return Err(Error::Corrupt);
        }
        Ok(route)
    }

    pub fn name(&self) -> &'a str {
        core::str::from_utf8(&self.data[self.name.0..self.name.1]).unwrap_or("")
    }

    pub fn points(&self) -> Points<'a> {
        Points {
            r: Reader {
                data: self.data,
                at: self.points_at,
            },
            left: self.info.count,
            last: Point {
                x: 0,
                y: 0,
                ele_dm: 0,
                along_dm: 0,
            },
        }
    }

    /// Position on the route's plane of a GNSS position, decimetres (the same integer projection as the points).
    pub fn project(&self, lat_e7: i32, lon_e7: i32) -> (i32, i32) {
        project(&self.info, lat_e7, lon_e7)
    }

    /// Where (`x`, `y`) is along the route: the nearest segment. With `last` (the previous result), segments from
    /// 500 m before to 2 km after it are preferred within 200 m, so that a loop or an out-and-back does not jump to
    /// the other pass.
    pub fn locate(&self, x: i32, y: i32, last: Option<Progress>) -> Progress {
        let mut best: Option<(i64, Progress)> = None;
        let mut near: Option<(i64, Progress)> = None;
        let mut points = self.points();
        let Some(mut a) = points.next() else {
            return Progress {
                along_dm: 0,
                off_dm: u32::MAX,
                segment: 0,
            };
        };
        for (i, b) in points.enumerate() {
            let (d2, along) = to_segment(a, b, x, y);
            let p = Progress {
                along_dm: along,
                off_dm: 0,
                segment: i as u32,
            };
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, p));
            }
            if let Some(l) = last
                && (l.along_dm.saturating_sub(5_000)..=l.along_dm + 20_000).contains(&along)
                && near.is_none_or(|(nd, _)| d2 < nd)
            {
                near = Some((d2, p));
            }
            a = b;
        }
        let (d2, mut p) = match (near, best) {
            (Some(n), _) if n.0 <= 2_000 * 2_000 => n,
            (_, Some(b)) => b,
            _ => unreachable!("a route has at least 2 points"),
        };
        p.off_dm = isqrt(d2 as u64) as u32;
        p
    }
}

/// Squared distance from (`x`, `y`) to segment `a`-`b`, and the distance along the route at the closest point.
fn to_segment(a: Point, b: Point, x: i32, y: i32) -> (i64, u32) {
    let (dx, dy) = ((b.x - a.x) as i64, (b.y - a.y) as i64);
    let (px, py) = ((x - a.x) as i64, (y - a.y) as i64);
    let len2 = dx * dx + dy * dy;
    // the closest point at a fraction t of the segment, in 1/65536
    let t = if len2 == 0 { 0 } else { ((px * dx + py * dy) * 65_536 / len2).clamp(0, 65_536) };
    let (cx, cy) = (dx * t / 65_536, dy * t / 65_536);
    let d2 = (px - cx) * (px - cx) + (py - cy) * (py - cy);
    let along = a.along_dm as i64 + (b.along_dm - a.along_dm) as i64 * t / 65_536;
    (d2, along as u32)
}

fn project(info: &Info, lat_e7: i32, lon_e7: i32) -> (i32, i32) {
    let x = ((lon_e7 as i64 - info.lon0_e7 as i64) * info.kx_q24 as i64) >> 24;
    let y = ((lat_e7 as i64 - info.lat0_e7 as i64) * info.ky_q24 as i64) >> 24;
    (x as i32, y as i32)
}

pub fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

pub struct Points<'a> {
    r: Reader<'a>,
    left: u32,
    last: Point,
}

impl Iterator for Points<'_> {
    type Item = Point;

    fn next(&mut self) -> Option<Point> {
        if self.left == 0 {
            return None;
        }
        let p = Point {
            x: self.last.x.wrapping_add(self.r.zigzag().ok()?),
            y: self.last.y.wrapping_add(self.r.zigzag().ok()?),
            ele_dm: self.last.ele_dm.wrapping_add(self.r.zigzag().ok()?),
            along_dm: self.last.along_dm.wrapping_add(self.r.varint().ok()?),
        };
        self.left -= 1;
        self.last = p;
        Some(p)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let b = self.data.get(self.at..self.at + N).ok_or(Error::Corrupt)?;
        self.at += N;
        Ok(b.try_into().unwrap())
    }

    fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.data.get(self.at..self.at + n).ok_or(Error::Corrupt)?;
        self.at += n;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.take()?))
    }

    fn varint(&mut self) -> Result<u32, Error> {
        let mut v = 0u32;
        for shift in (0..35).step_by(7) {
            let b = self.u8()?;
            v |= ((b & 0x7f) as u32).checked_shl(shift).ok_or(Error::Corrupt)?;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Error::Corrupt)
    }

    fn zigzag(&mut self) -> Result<i32, Error> {
        let v = self.varint()?;
        Ok((v >> 1) as i32 ^ -((v & 1) as i32))
    }
}

/// CRC-16/CCITT-FALSE.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}
