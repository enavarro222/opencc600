//! Background map building, off the device: features (OSM ways, in degrees ×1e7) projected on the route's plane,
//! kept in the cells of a corridor around the route, clipped to them, simplified for each level, encoded.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;

use crate::build::{varint, zigzag};
use crate::map::{DETAIL, MAGIC, MAX_POINTS, OVERVIEW, VERSION, class};
use crate::{Route, crc16};

/// A line or an area (a closed ring) of a class of [`crate::map::class`], latitude and longitude ×1e7.
#[derive(Clone, Debug, PartialEq)]
pub struct MapFeature {
    pub class: u8,
    pub points: Vec<(i32, i32)>,
}

/// What went into each level: bytes and points by class.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapStats {
    pub cells: [usize; 2],
    pub bytes: [[usize; class::COUNT]; 2],
    pub points: [[usize; class::COUNT]; 2],
}

/// Cells of the detail level, and the corridor's grain.
const DETAIL_CELL_M: f64 = 500.0;
/// Cells of the overview.
const OVERVIEW_CELL_M: f64 = 4000.0;
/// Simplification of the detail: under a pixel at the closest zoom (8 m a pixel).
const DETAIL_TOLERANCE_M: f64 = 2.0;
/// Contour lines come from a terrain model of ~25 m: no need for more.
const CONTOUR_TOLERANCE_M: f64 = 5.0;
/// The page's map area and the margins of the whole-route view (crates/ui, `Transform::fit`).
const PAGE_W_PX: f64 = 200.0;
const PAGE_H_PX: f64 = 158.0;
/// Classes left out of the overview: too fine for its scale.
const OVERVIEW_SKIPS: [u8; 5] = [class::PATH, class::SERVICE, class::STREAM, class::CONTOUR, class::CONTOUR_MAJOR];

/// The map of the route `route_file` (the bytes the device holds): the features within `corridor_m` of the route
/// (by cells of 500 m), a detail level and an overview level.
pub fn build_map(route_file: &[u8], features: &[MapFeature], corridor_m: f64) -> (Vec<u8>, MapStats) {
    let route = Route::parse(route_file).expect("a route file");
    let to_m = |lat: i32, lon: i32| {
        let (x, y) = route.project(lat, lon);
        (x as f64 / 10.0, y as f64 / 10.0)
    };
    // the cells of the corridor: within corridor_m of a point of the route, every 50 m along it
    let mut kept = BTreeSet::new();
    let mut points = route.points();
    let mut a = points.next().expect("2 points");
    let reach = libm::ceil(corridor_m / DETAIL_CELL_M) as i32 + 1;
    for b in points {
        let (ax, ay, bx, by) = (a.x as f64 / 10.0, a.y as f64 / 10.0, b.x as f64 / 10.0, b.y as f64 / 10.0);
        let n = libm::ceil(libm::hypot(bx - ax, by - ay) / 50.0).max(1.0) as i32;
        for k in 0..=n {
            let f = k as f64 / n as f64;
            let (x, y) = (ax + (bx - ax) * f, ay + (by - ay) * f);
            let (cx, cy) = (cell_of(x, DETAIL_CELL_M), cell_of(y, DETAIL_CELL_M));
            for j in cy - reach..=cy + reach {
                for i in cx - reach..=cx + reach {
                    // the distance from the point to the cell's square
                    let (x0, y0) = (i as f64 * DETAIL_CELL_M, j as f64 * DETAIL_CELL_M);
                    let dx = (x0 - x).max(x - x0 - DETAIL_CELL_M).max(0.0);
                    let dy = (y0 - y).max(y - y0 - DETAIL_CELL_M).max(0.0);
                    if dx * dx + dy * dy <= corridor_m * corridor_m {
                        kept.insert((i, j));
                    }
                }
            }
        }
        a = b;
    }
    // the overview's scale: the whole route in the page
    let [bx0, by0, bx1, by1] = route.info.bbox;
    let m_per_px = (((bx1 - bx0) as f64 / 10.0 / PAGE_W_PX).max((by1 - by0) as f64 / 10.0 / PAGE_H_PX)).max(1.0);

    // each feature clipped to the kept cells it crosses
    let mut cells: [BTreeMap<(i32, i32), Vec<(u8, Vec<(i32, i32)>)>>; 2] = [BTreeMap::new(), BTreeMap::new()];
    let mut stats = MapStats::default();
    for f in features {
        let pts: Vec<(f64, f64)> = f.points.iter().map(|&(lat, lon)| to_m(lat, lon)).collect();
        if pts.len() < 2 {
            continue;
        }
        let area = class::is_area(f.class);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in &pts {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        for cy in cell_of(y0, DETAIL_CELL_M)..=cell_of(y1, DETAIL_CELL_M) {
            for cx in cell_of(x0, DETAIL_CELL_M)..=cell_of(x1, DETAIL_CELL_M) {
                if !kept.contains(&(cx, cy)) {
                    continue;
                }
                let bx = (cx as f64 * DETAIL_CELL_M, cy as f64 * DETAIL_CELL_M);
                let b = (bx.0, bx.1, bx.0 + DETAIL_CELL_M, bx.1 + DETAIL_CELL_M);
                let pieces = if area { clip_ring(&pts, b).into_iter().collect() } else { clip_line(&pts, b) };
                for piece in pieces {
                    for level in [DETAIL, OVERVIEW] {
                        let (tol, size) = match level {
                            DETAIL if matches!(f.class, class::CONTOUR | class::CONTOUR_MAJOR) => (CONTOUR_TOLERANCE_M, DETAIL_CELL_M),
                            DETAIL => (DETAIL_TOLERANCE_M, DETAIL_CELL_M),
                            _ => (m_per_px / 2.0, OVERVIEW_CELL_M),
                        };
                        if level == OVERVIEW && (OVERVIEW_SKIPS.contains(&f.class) || small(&piece, area, m_per_px)) {
                            continue;
                        }
                        let Some(q) = encode_points(&piece, tol, area) else { continue };
                        let key = (cell_of(bx.0 + 1.0, size), cell_of(bx.1 + 1.0, size));
                        cells[level].entry(key).or_default().push((f.class, q));
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&crc16(route_file).to_le_bytes());
    out.push(2);
    let header_at = out.len();
    out.resize(header_at + 2 * 10, 0);
    for level in [DETAIL, OVERVIEW] {
        let size = if level == DETAIL { DETAIL_CELL_M } else { OVERVIEW_CELL_M };
        // sorted by row then column, the order the device looks them up in
        let mut keys: Vec<_> = cells[level].keys().copied().collect();
        keys.sort_by_key(|&(x, y)| (y, x));
        let index_at = out.len();
        out.resize(index_at + keys.len() * 8 + 4, 0);
        for (i, key) in keys.iter().enumerate() {
            let at = out.len() as u32;
            let e = index_at + i * 8;
            out[e..e + 2].copy_from_slice(&(key.0 as i16).to_le_bytes());
            out[e + 2..e + 4].copy_from_slice(&(key.1 as i16).to_le_bytes());
            out[e + 4..e + 8].copy_from_slice(&at.to_le_bytes());
            let origin = ((key.0 as f64 * size) as i32, (key.1 as f64 * size) as i32);
            let mut list = cells[level][key].clone();
            list.sort_by_key(|(c, _)| *c);
            for (c, q) in list {
                let mut body = Vec::new();
                let mut last = origin;
                for &p in &q {
                    zigzag(&mut body, p.0 - last.0);
                    zigzag(&mut body, p.1 - last.1);
                    last = p;
                }
                let start = out.len();
                out.push(c);
                varint(&mut out, q.len() as u32);
                varint(&mut out, body.len() as u32);
                out.extend_from_slice(&body);
                stats.bytes[level][c as usize] += out.len() - start;
                stats.points[level][c as usize] += q.len();
            }
        }
        let end = out.len() as u32;
        let e = index_at + keys.len() * 8;
        out[e..e + 4].copy_from_slice(&end.to_le_bytes());
        let h = header_at + level * 10;
        out[h..h + 2].copy_from_slice(&(size as u16).to_le_bytes());
        out[h + 2..h + 6].copy_from_slice(&(keys.len() as u32).to_le_bytes());
        out[h + 6..h + 10].copy_from_slice(&(index_at as u32).to_le_bytes());
        stats.cells[level] = keys.len();
    }
    let crc = crc16(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    (out, stats)
}

fn cell_of(v: f64, size: f64) -> i32 {
    libm::floor(v / size) as i32
}

/// A piece under a few pixels of the overview: left out.
fn small(p: &[(f64, f64)], area: bool, m_per_px: f64) -> bool {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in p {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let (w, h) = ((x1 - x0) / m_per_px, (y1 - y0) / m_per_px);
    if area { w * h < 4.0 } else { w.max(h) < 2.0 }
}

/// Simplified and rounded to the metre; at most [`MAX_POINTS`] (simplified further if needed); `None` if nothing is
/// left.
fn encode_points(p: &[(f64, f64)], tolerance: f64, area: bool) -> Option<Vec<(i32, i32)>> {
    let mut tol = tolerance;
    loop {
        let keep = douglas_peucker(p, tol);
        let mut q: Vec<(i32, i32)> = Vec::new();
        for (i, &(x, y)) in p.iter().enumerate() {
            let r = (libm::round(x) as i32, libm::round(y) as i32);
            if keep[i] && q.last() != Some(&r) {
                q.push(r);
            }
        }
        if q.len() < if area { 3 } else { 2 } {
            return None;
        }
        if q.len() <= MAX_POINTS {
            return Some(q);
        }
        tol *= 1.5;
    }
}

fn douglas_peucker(p: &[(f64, f64)], tol: f64) -> Vec<bool> {
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    // a ring starts where it ends: split at its farthest point from the start
    let mut stack = Vec::new();
    let far = (1..p.len()).max_by(|&i, &j| d2(p[0], p[i]).total_cmp(&d2(p[0], p[j]))).unwrap_or(0);
    if d2(p[0], p[p.len() - 1]) < 1e-6 && far > 0 && far < p.len() - 1 {
        keep[far] = true;
        stack.push((0, far));
        stack.push((far, p.len() - 1));
    } else {
        stack.push((0, p.len() - 1));
    }
    while let Some((a, b)) = stack.pop() {
        let (mut best, mut dmax) = (0, -1.0);
        for i in a + 1..b {
            let d = to_segment(p[i], p[a], p[b]);
            if d > dmax {
                (best, dmax) = (i, d);
            }
        }
        if dmax > tol {
            keep[best] = true;
            stack.push((a, best));
            stack.push((best, b));
        }
    }
    keep
}

fn d2(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1)
}

fn to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    libm::sqrt(d2(p, (a.0 + dx * t, a.1 + dy * t)))
}

/// The runs of a polyline inside the box (x0, y0, x1, y1), each segment clipped (Liang-Barsky).
fn clip_line(p: &[(f64, f64)], (x0, y0, x1, y1): (f64, f64, f64, f64)) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut inside = true;
        for (q, r) in [(-dx, a.0 - x0), (dx, x1 - a.0), (-dy, a.1 - y0), (dy, y1 - a.1)] {
            if q == 0.0 {
                if r < 0.0 {
                    inside = false;
                }
            } else {
                let t = r / q;
                if q < 0.0 { t0 = t0.max(t) } else { t1 = t1.min(t) }
            }
        }
        if !inside || t0 > t1 {
            if cur.len() > 1 {
                runs.push(core::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        let (s, e) = ((a.0 + dx * t0, a.1 + dy * t0), (a.0 + dx * t1, a.1 + dy * t1));
        if cur.last() != Some(&s) {
            if cur.len() > 1 {
                runs.push(core::mem::take(&mut cur));
            }
            cur = vec![s];
        }
        cur.push(e);
        if t1 < 1.0 {
            runs.push(core::mem::take(&mut cur));
        }
    }
    if cur.len() > 1 {
        runs.push(cur);
    }
    runs
}

/// A ring clipped to the box (Sutherland-Hodgman), closed; `None` if nothing is inside.
fn clip_ring(p: &[(f64, f64)], (x0, y0, x1, y1): (f64, f64, f64, f64)) -> Option<Vec<(f64, f64)>> {
    let mut ring: Vec<(f64, f64)> = p.to_vec();
    if ring.first() == ring.last() {
        ring.pop();
    }
    // each edge of the box: inside test, and the crossing of a segment with it
    type Side = (fn((f64, f64), f64) -> bool, fn((f64, f64), (f64, f64), f64) -> (f64, f64), f64);
    let cross_x = |a: (f64, f64), b: (f64, f64), x: f64| (x, a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0));
    let cross_y = |a: (f64, f64), b: (f64, f64), y: f64| (a.0 + (b.0 - a.0) * (y - a.1) / (b.1 - a.1), y);
    let sides: [Side; 4] = [
        (|p, v| p.0 >= v, cross_x, x0),
        (|p, v| p.0 <= v, cross_x, x1),
        (|p, v| p.1 >= v, cross_y, y0),
        (|p, v| p.1 <= v, cross_y, y1),
    ];
    for (inside, cross, v) in sides {
        let mut next = Vec::with_capacity(ring.len() + 4);
        for i in 0..ring.len() {
            let (a, b) = (ring[(i + ring.len() - 1) % ring.len()], ring[i]);
            match (inside(a, v), inside(b, v)) {
                (true, true) => next.push(b),
                (true, false) => next.push(cross(a, b, v)),
                (false, true) => {
                    next.push(cross(a, b, v));
                    next.push(b);
                }
                (false, false) => {}
            }
        }
        ring = next;
        if ring.len() < 3 {
            return None;
        }
    }
    let first = ring[0];
    ring.push(first);
    Some(ring)
}
