//! The map page: the route around the rider, heading up, the part done in grey, or the whole route north up; what is
//! left of the route and the speed on top. Under the route, the background map when there is one
//! ([`route::map::Map`]): areas filled in spans, lines one to three pixels wide, not anti-aliased.
//!
//! The route is drawn as anti-aliased capsules, clipped to the page first: only the segments in view cost.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyleBuilder, Rectangle};
use route::Route;
use route::map::{self, Map, class};

use crate::draw::{BOTTOM, TOP, Theme, W, km, kmh, rect, rounded};
use crate::fonts::{self, Align, Line as Fmt, Style};
use crate::View;

/// The rider on the page: centred, low (more of what is ahead).
const ME: (f32, f32) = (120.0, 212.0);
/// The scale bar, pixels for the zoom's metres.
pub(crate) const SCALE_PX: f32 = 60.0;
/// Width of the route line.
const LINE: f32 = 5.0;
/// The squared distances from the centre line under which an edge pixel's coverage reaches 1/15, 2/15... 15/15:
/// the coverage counted without a square root (the `libm` one is slow on the Xtensa).
const EDGE2: [f32; 15] = {
    let mut t = [0.0; 15];
    let mut k = 0;
    while k < 15 {
        let s = LINE / 2.0 + 0.5 - (k as f32 + 0.5) / 15.0;
        t[k] = s * s;
        k += 1;
    }
    t
};

pub(crate) fn draw<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    zoom_m: Option<u32>,
    v: &View,
    route: Option<&Route<'_>>,
    bg_map: Option<&Map<'_>>,
    t: &Theme,
) -> Result<(), D::Error> {
    let (Some(route), Some(rv)) = (route, v.route.as_ref()) else {
        let s = Style::new(&fonts::ROW, t.mute, t.bg);
        fonts::text(d, &s, "Pas de trace", W / 2, (TOP + BOTTOM) / 2, Align::Center)?;
        return Ok(());
    };
    let area = rect(0, TOP + 1, W - 1, BOTTOM - 1);
    let along = rv.progress.map(|p| p.along_dm);
    // the whole route north up: that zoom step, or without a fix
    let (view, around) = match (rv.me, zoom_m) {
        (Some(me), Some(z)) => (Transform::around(me, rv.heading_cdeg.unwrap_or(0), z), Some(z)),
        _ => (Transform::fit(&route.info.bbox, area), None),
    };
    if let Some(m) = bg_map {
        background(d, m, &view, around, t, area)?;
    }
    line(d, route, &view, along, (t.done, t.route), t.bg, area)?;
    match (around, rv.me) {
        (Some(_), _) => me_marker(d, t)?,
        // on the whole route: where the rider is, a dot ringed in the background colour
        (None, Some(me)) => {
            let (x, y) = view.apply(me.0, me.1);
            Circle::with_center(Point::new(x as i32, y as i32), 11)
                .into_styled(PrimitiveStyleBuilder::new().fill_color(t.me).stroke_color(t.bg).stroke_width(2).build())
                .draw(d)?;
        }
        _ => {}
    }
    // what is left, the speed
    let left = match (along, rv.me) {
        (Some(a), Some(_)) => Fmt::new(format_args!("{} km", km(rv.length_dm.saturating_sub(a) * 10).as_str())),
        _ => Fmt::new(format_args!("{} km", km(rv.length_dm * 10).as_str())),
    };
    let label = Style::new(&fonts::LABEL_TINY, t.mute, t.bg).spaced(60);
    let value = Style::new(&fonts::HEADING_SMALL, t.fg, t.bg);
    let box_w = fonts::HEADING_SMALL.width("000,0 km", 0) + 12;
    overlay(d, rect(8, 36, 8 + box_w, 80), t)?;
    fonts::text(d, &label, "RESTE", 14, 47, Align::Left)?;
    fonts::value(d, &value, left.as_str(), "", 14, 65, Align::Left)?;
    // the unit in the label, as on the other pages
    let speed_w = fonts::HEADING_SMALL.width("00,0", 0).max(fonts::LABEL_TINY.width("VITESSE KM/H", 60)) + 12;
    overlay(d, rect(W - 8 - speed_w, 36, W - 8, 80), t)?;
    fonts::text(d, &label, "VITESSE KM/H", W - 14, 47, Align::Right)?;
    fonts::value(d, &value, kmh(v.speed_mm_s).as_str(), "00,0", W - 14, 65, Align::Right)?;
    Ok(())
}

/// A white box with a thin border over the map.
fn overlay<D: DrawTarget<Color = Rgb565>>(d: &mut D, r: Rectangle, t: &Theme) -> Result<(), D::Error> {
    rounded(d, r, 4, t.rule, t.bg)?;
    let inner = Rectangle::new(r.top_left + Point::new(1, 1), r.size - Size::new(2, 2));
    rounded(d, inner, 3, t.bg, t.rule)
}

/// From the route's plane (decimetres) to the page (pixels).
struct Transform {
    origin: (f32, f32),
    scale: f32,
    cos: f32,
    sin: f32,
    at: (f32, f32),
}

impl Transform {
    /// Heading up around the rider: `zoom_m` metres are [`SCALE_PX`] pixels.
    fn around(me: (i32, i32), heading_cdeg: u16, zoom_m: u32) -> Self {
        let a = (heading_cdeg as f32 / 100.0).to_radians();
        Self {
            origin: (me.0 as f32, me.1 as f32),
            scale: SCALE_PX / (zoom_m as f32 * 10.0),
            cos: libm::cosf(a),
            sin: libm::sinf(a),
            at: ME,
        }
    }

    /// North up, the bounding box in the area with a margin.
    fn fit(bbox: &[i32; 4], area: Rectangle) -> Self {
        let (w, h) = ((bbox[2] - bbox[0]).max(1) as f32, (bbox[3] - bbox[1]).max(1) as f32);
        let (aw, ah) = (area.size.width as f32 - 40.0, area.size.height as f32 - 90.0);
        let c = area.center();
        Self {
            origin: ((bbox[0] + bbox[2]) as f32 / 2.0, (bbox[1] + bbox[3]) as f32 / 2.0),
            scale: (aw / w).min(ah / h),
            cos: 1.0,
            sin: 0.0,
            at: (c.x as f32, c.y as f32 + 20.0),
        }
    }

    fn apply(&self, x: i32, y: i32) -> (f32, f32) {
        let (dx, dy) = ((x as f32 - self.origin.0) * self.scale, (y as f32 - self.origin.1) * self.scale);
        // heading up: the world turned by minus the heading (x east, y north, the page's y down)
        let u = dx * self.cos - dy * self.sin;
        let v = dx * self.sin + dy * self.cos;
        (self.at.0 + u, self.at.1 - v)
    }
}

/// Line widths (pixels) and dashes (pixels on, off; 0: solid) of the map's classes ([`class`]); the areas have none.
const STYLE: [(u8, u8, u8); class::COUNT] = [
    (0, 0, 0), (0, 0, 0), (0, 0, 0), (1, 0, 0), (1, 0, 0), (1, 0, 0),
    (2, 0, 0), (1, 2, 2), (1, 4, 3), (1, 0, 0), (2, 0, 0), (3, 0, 0),
];
/// From this zoom out, the finest lines are left out around the rider.
const FINE_UNTIL_M: u32 = 2000;
/// Half the diagonal seen from the rider, pixels: the cells within it are read.
const REACH_PX: f32 = 230.0;

/// The background map under the route: around the rider (`around`, the zoom's metres) the detail level's cells in
/// view, else the overview's, all of them. Areas first, then the lines from the thinnest.
fn background<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    m: &Map<'_>,
    view: &Transform,
    around: Option<u32>,
    t: &Theme,
    area: Rectangle,
) -> Result<(), D::Error> {
    let level = match around {
        Some(_) => m.level(map::DETAIL),
        None => m.level(map::OVERVIEW).or(m.level(map::DETAIL)),
    };
    let Some(level) = level else { return Ok(()) };
    // the columns and rows in view: around the rider, those within reach; the whole route: all
    let (from, to) = match around {
        Some(_) => {
            let reach_m = REACH_PX / (view.scale * 10.0);
            let (x, y) = (view.origin.0 / 10.0, view.origin.1 / 10.0);
            let cell = level.cell_m as f32;
            let c = |v: f32| libm::floorf(v / cell) as i32;
            ((c(x - reach_m), c(y - reach_m)), (c(x + reach_m), c(y + reach_m)))
        }
        None => match m.rows(level) {
            Some((y0, y1)) => ((i16::MIN as i32, y0), (i16::MAX as i32, y1)),
            None => return Ok(()),
        },
    };
    let skip_fine = around.is_some_and(|z| z >= FINE_UNTIL_M);
    let mut pts = [(0.0f32, 0.0f32); map::MAX_POINTS];
    let mut slopes = [0.0f32; map::MAX_POINTS];
    let (ax0, ay0) = (area.top_left.x as f32, area.top_left.y as f32);
    let (ax1, ay1) = (ax0 + area.size.width as f32, ay0 + area.size.height as f32);
    for areas in [true, false] {
        for cell in m.cells(level, from, to) {
            for f in cell.features() {
                if class::is_area(f.class) != areas
                    || (skip_fine && matches!(f.class, class::PATH | class::SERVICE | class::STREAM | class::CONTOUR))
                {
                    continue;
                }
                // projected once, with their box: a feature out of the page costs only this
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                let mut n = 0;
                for (x, y) in f.points() {
                    let p = view.apply(x * 10, y * 10);
                    (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
                    pts[n] = p;
                    n += 1;
                }
                if n < 2 || x1 < ax0 || x0 > ax1 || y1 < ay0 || y0 > ay1 {
                    continue;
                }
                let c = t.map[f.class as usize];
                if areas {
                    fill(d, &pts[..n], &mut slopes[..n], c, area)?;
                } else {
                    let (width, on, off) = STYLE[f.class as usize];
                    let clip = (ax0, ay0, ax1 - 1.0, ay1 - 1.0);
                    let mut phase = 0u8;
                    for w in pts[..n].windows(2) {
                        if let Some((p, q)) = clip_segment(w[0], w[1], clip) {
                            thin_line(d, p, q, width, (on, off), &mut phase, c, area)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// A ring filled row by row (even-odd), in spans: where each row's centre crosses its edges. The slope of each edge
/// is worked out once (the Xtensa FPU has no divide).
fn fill<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    pts: &[(f32, f32)],
    slopes: &mut [f32],
    c: Rgb565,
    area: Rectangle,
) -> Result<(), D::Error> {
    let n = pts.len();
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for i in 0..n {
        let (a, b) = (pts[(i + n - 1) % n], pts[i]);
        slopes[i] = if a.1 != b.1 { (b.0 - a.0) / (b.1 - a.1) } else { 0.0 };
        (y0, y1) = (y0.min(b.1), y1.max(b.1));
    }
    let top = area.top_left.y;
    let bottom = top + area.size.height as i32 - 1;
    let (left, right) = (area.top_left.x, area.top_left.x + area.size.width as i32 - 1);
    let mut xs = [0.0f32; 32];
    for y in (y0 as i32).max(top)..=(y1 as i32 + 1).min(bottom) {
        let yc = y as f32 + 0.5;
        let mut k = 0;
        for i in 0..n {
            let (a, b) = (pts[(i + n - 1) % n], pts[i]);
            if (a.1 <= yc) != (b.1 <= yc) && k < xs.len() {
                // in order as they come
                let x = a.0 + (yc - a.1) * slopes[i];
                let mut j = k;
                while j > 0 && xs[j - 1] > x {
                    xs[j] = xs[j - 1];
                    j -= 1;
                }
                xs[j] = x;
                k += 1;
            }
        }
        for span in xs[..k].as_chunks::<2>().0 {
            let x0 = (libm::ceilf(span[0] - 0.5) as i32).max(left);
            let x1 = (libm::floorf(span[1] - 0.5) as i32).min(right);
            if x0 <= x1 {
                d.fill_solid(&Rectangle::new(Point::new(x0, y), Size::new((x1 - x0 + 1) as u32, 1)), c)?;
            }
        }
    }
    Ok(())
}

/// A line `width` pixels wide (1 to 3, across its main axis), not anti-aliased (Bresenham), dashed when `on` is not
/// 0: `phase` carries the dash from one segment to the next.
#[allow(clippy::too_many_arguments)]
fn thin_line<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    p: (f32, f32),
    q: (f32, f32),
    width: u8,
    (on, off): (u8, u8),
    phase: &mut u8,
    c: Rgb565,
    area: Rectangle,
) -> Result<(), D::Error> {
    let (mut x, mut y) = (p.0 as i32, p.1 as i32);
    let (x1, y1) = (q.0 as i32, q.1 as i32);
    let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
    let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
    // the extra pixels across the main axis
    let across: &[i32] = match width {
        0 | 1 => &[0],
        2 => &[0, 1],
        _ => &[-1, 0, 1],
    };
    let steep = -dy > dx;
    let mut err = dx + dy;
    let mut done = false;
    let period = on + off;
    let pixels = core::iter::from_fn(move || {
        if done {
            return None;
        }
        let at = (x, y);
        let draw = period == 0 || *phase < on;
        if period > 0 {
            *phase = (*phase + 1) % period;
        }
        if x == x1 && y == y1 {
            done = true;
        } else {
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
        Some((at, draw))
    })
    .filter(|&(_, draw)| draw)
    .flat_map(|((x, y), _)| {
        across.iter().map(move |&o| if steep { Point::new(x + o, y) } else { Point::new(x, y + o) })
    })
    .filter(|p| area.contains(*p))
    .map(|p| Pixel(p, c));
    d.draw_iter(pixels)
}

/// The route: the part done (before `along`) and the part ahead, in their colours (`done`, `ahead`). Each point is
/// projected once a pass, segments out of the page are dropped before anything else, and the points closer than
/// 1.5 pixels to the last one drawn join the next segment (most of them when zoomed out).
fn line<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    route: &Route<'_>,
    view: &Transform,
    along: Option<u32>,
    (done, ahead): (Rgb565, Rgb565),
    bg: Rgb565,
    area: Rectangle,
) -> Result<(), D::Error> {
    let along = along.unwrap_or(0);
    let clip = (
        area.top_left.x as f32 - LINE,
        area.top_left.y as f32 - LINE,
        (area.top_left.x + area.size.width as i32) as f32 + LINE,
        (area.top_left.y + area.size.height as i32) as f32 + LINE,
    );
    // edges first, then the full pixels: a joint never shows the edge of the next segment over the line
    for full in [false, true] {
        let mut points = route.points();
        let Some(mut a) = points.next() else { return Ok(()) };
        let mut pa = view.apply(a.x, a.y);
        let mut left = route.info.count - 1;
        for b in points {
            left -= 1;
            let pb = view.apply(b.x, b.y);
            let split = (a.along_dm..b.along_dm).contains(&along);
            let short = (pb.0 - pa.0) * (pb.0 - pa.0) + (pb.1 - pa.1) * (pb.1 - pa.1) < 2.25;
            if short && left > 0 && !split {
                continue;
            }
            let out = (pa.0 < clip.0 && pb.0 < clip.0)
                || (pa.0 > clip.2 && pb.0 > clip.2)
                || (pa.1 < clip.1 && pb.1 < clip.1)
                || (pa.1 > clip.3 && pb.1 > clip.3);
            if !out {
                // the done part then the part ahead, split where the rider is
                let (pm, cut) = if split {
                    let f = (along - a.along_dm) as f32 / (b.along_dm - a.along_dm).max(1) as f32;
                    ((pa.0 + (pb.0 - pa.0) * f, pa.1 + (pb.1 - pa.1) * f), true)
                } else {
                    (pb, false)
                };
                let parts = if cut {
                    [(pa, pm, done), (pm, pb, ahead)]
                } else if b.along_dm <= along {
                    [(pa, pb, done), (pb, pb, done)]
                } else {
                    [(pa, pb, ahead), (pb, pb, ahead)]
                };
                for (p, q, c) in parts {
                    if p != q
                        && let Some((p, q)) = clip_segment(p, q, clip)
                    {
                        capsule(d, p, q, c, bg, area, full)?;
                    }
                }
            }
            a = b;
            pa = pb;
        }
    }
    Ok(())
}

/// Cohen-Sutherland: the part of segment p-q inside the box, if any.
fn clip_segment(
    mut p: (f32, f32),
    mut q: (f32, f32),
    (x0, y0, x1, y1): (f32, f32, f32, f32),
) -> Option<((f32, f32), (f32, f32))> {
    let code = |(x, y): (f32, f32)| {
        (x < x0) as u8 | ((x > x1) as u8) << 1 | ((y < y0) as u8) << 2 | ((y > y1) as u8) << 3
    };
    for _ in 0..8 {
        let (cp, cq) = (code(p), code(q));
        if cp | cq == 0 {
            return Some((p, q));
        }
        if cp & cq != 0 {
            return None;
        }
        let out = if cp != 0 { cp } else { cq };
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let r = if out & 1 != 0 {
            (x0, p.1 + dy * (x0 - p.0) / dx)
        } else if out & 2 != 0 {
            (x1, p.1 + dy * (x1 - p.0) / dx)
        } else if out & 4 != 0 {
            (p.0 + dx * (y0 - p.1) / dy, y0)
        } else {
            (p.0 + dx * (y1 - p.1) / dy, y1)
        };
        if out == cp { p = r } else { q = r }
    }
    None
}

/// A segment [`LINE`] pixels wide with round ends, its edge anti-aliased into `bg`; only the full pixels or only
/// the edge ones (`full`). Only the band around the segment is visited: along its main axis, the few pixels on
/// each side of the centre line. Along a row (or column) the projection on the segment and the distance across it
/// change by a constant from one pixel to the next: no division, no conversion, no square root for each pixel (the
/// Xtensa FPU has no divide and is slow on dependent operations).
fn capsule<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    p: (f32, f32),
    q: (f32, f32),
    c: Rgb565,
    bg: Rgb565,
    area: Rectangle,
    full: bool,
) -> Result<(), D::Error> {
    let r = LINE / 2.0;
    let (ax0, ay0) = (area.top_left.x, area.top_left.y);
    let (ax1, ay1) = (ax0 + area.size.width as i32 - 1, ay0 + area.size.height as i32 - 1);
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    let len2 = dx * dx + dy * dy;
    let len = libm::sqrtf(len2);
    let inv_len2 = if len2 == 0.0 { 0.0 } else { 1.0 / len2 };
    let (inner2, outer2) = ((r - 0.5) * (r - 0.5), (r + 0.5) * (r + 0.5));
    // walk the main axis (y for a steep segment), the band across it
    let steep = dy.abs() >= dx.abs();
    let (along, across) = if steep { (dy, dx) } else { (dx, dy) };
    let (a0, a1) = if steep { (p.1, q.1) } else { (p.0, q.0) };
    let (lo, hi) = (a0.min(a1) - r - 1.0, a0.max(a1) + r + 1.0);
    let (lo, hi) = if steep {
        (lo.max(ay0 as f32) as i32, hi.min(ay1 as f32) as i32)
    } else {
        (lo.max(ax0 as f32) as i32, hi.min(ax1 as f32) as i32)
    };
    // half the band's width across the main axis, plus a pixel
    let half = if along.abs() > 0.01 { (r + 1.0) * len / along.abs() } else { r + 1.0 } + 1.0;
    // and no wider than the segment's box (a short segment's band is mostly empty)
    let (c0, c1) = if steep { (p.0, q.0) } else { (p.1, q.1) };
    let (b0, b1) = if steep { (ax0, ax1) } else { (ay0, ay1) };
    let (b0, b1) = (b0.max((c0.min(c1) - r - 1.0) as i32), b1.min((c0.max(c1) + r + 1.0) as i32));
    let inv_along = if along.abs() > 0.01 { 1.0 / along } else { 0.0 };
    // one pixel across: (sx, sy), the projection (times len2) and the cross product change by these
    let (sx, sy) = if steep { (1.0, 0.0) } else { (0.0, 1.0) };
    let (step_dot, step_cross) = (sx * dx + sy * dy, sx * dy - sy * dx);
    let pixels = (lo..=hi).flat_map(move |m| {
        // where the centre line crosses this row (or column), held at the segment's ends
        let t = ((m as f32 + 0.5 - a0) * inv_along).clamp(0.0, 1.0);
        let centre = if steep { p.0 } else { p.1 } + across * t;
        let from = ((centre - half) as i32).max(b0);
        let to = ((centre + half) as i32).min(b1);
        // the first pixel, from p
        let (x, y) = if steep { (from, m) } else { (m, from) };
        let (mut px, mut py) = (x as f32 + 0.5 - p.0, y as f32 + 0.5 - p.1);
        let (mut dot, mut cross) = (px * dx + py * dy, px * dy - py * dx);
        (from..=to).filter_map(move |n| {
            // the squared distance to the segment: to p, to q, or across
            let d2 = if dot <= 0.0 {
                px * px + py * py
            } else if dot >= len2 {
                (px - dx) * (px - dx) + (py - dy) * (py - dy)
            } else {
                cross * cross * inv_len2
            };
            px += sx;
            py += sy;
            dot += step_dot;
            cross += step_cross;
            let a = if d2 <= inner2 {
                15
            } else if d2 >= outer2 {
                0
            } else {
                EDGE2.iter().filter(|&&e| d2 <= e).count() as u8
            };
            let (x, y) = if steep { (n, m) } else { (m, n) };
            (if full { a == 15 } else { a > 0 && a < 15 }).then(|| Pixel(Point::new(x, y), fonts::blend(bg, c, a)))
        })
    });
    d.draw_iter(pixels)
}

/// The rider: a white disc with a blue ring and the arrow pointing up (the heading), from [`MARKER`].
fn me_marker<D: DrawTarget<Color = Rgb565>>(d: &mut D, t: &Theme) -> Result<(), D::Error> {
    let (x0, y0) = (ME.0 as i32 - MARKER_HALF, ME.1 as i32 - MARKER_HALF);
    let pixels = MARKER.iter().enumerate().flat_map(|(j, row)| {
        row.iter().enumerate().filter_map(move |(i, &(layer, a))| {
            let c = match layer {
                1 | 3 => fonts::blend(t.bg, t.me, a),
                2 => fonts::blend(t.me, t.bg, a),
                _ => return None,
            };
            Some(Pixel(Point::new(x0 + i as i32, y0 + j as i32), c))
        })
    });
    d.draw_iter(pixels)
}

const MARKER_HALF: i32 = 12;
const MARKER_SIZE: usize = 2 * MARKER_HALF as usize + 1;
/// The rider's marker, worked out at build time (it took 6 ms a frame on the device): for each pixel of its box, the
/// last layer over it (1 the blue disc, 2 the white one inside, 3 the arrow, 0 none) and that layer's coverage, 0 to
/// 15, sampled 4x4 a pixel as [`crate::draw::shape`] does.
const MARKER: [[(u8, u8); MARKER_SIZE]; MARKER_SIZE] = {
    let (cx, cy) = (ME.0, ME.1);
    // the prototype's arrow: a tip, two back corners and a notch, two triangles
    let (tip, left, notch, right) = ((cx, cy - 8.0), (cx - 6.0, cy + 6.0), (cx, cy + 3.0), (cx + 6.0, cy + 6.0));
    const fn side(p: (f32, f32), q: (f32, f32), x: f32, y: f32) -> f32 {
        (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)
    }
    const fn tri(a: (f32, f32), b: (f32, f32), c: (f32, f32), x: f32, y: f32) -> bool {
        let (s1, s2, s3) = (side(a, b, x, y), side(b, c, x, y), side(c, a, x, y));
        (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
    }
    let mut m = [[(0u8, 0u8); MARKER_SIZE]; MARKER_SIZE];
    let mut j = 0;
    while j < MARKER_SIZE {
        let mut i = 0;
        while i < MARKER_SIZE {
            let (px, py) = ((cx as i32 - MARKER_HALF + i as i32) as f32, (cy as i32 - MARKER_HALF + j as i32) as f32);
            let mut n = [0u8; 3];
            let mut s = 0;
            while s < 16 {
                let (x, y) = (px + ((s % 4) as f32 + 0.5) / 4.0, py + ((s / 4) as f32 + 0.5) / 4.0);
                let d2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
                n[0] += (d2 <= 11.0 * 11.0) as u8;
                n[1] += (d2 <= 9.0 * 9.0) as u8;
                n[2] += (tri(tip, left, notch, x, y) || tri(tip, notch, right, x, y)) as u8;
                s += 1;
            }
            let mut k = 3;
            while k > 0 {
                if n[k - 1] > 0 {
                    m[j][i] = (k as u8, n[k - 1] * 15 / 16);
                    break;
                }
                k -= 1;
            }
            i += 1;
        }
        j += 1;
    }
    m
};
