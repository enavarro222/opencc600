use super::*;

// --- primitives --------------------------------------------------------------------------------------------------

pub(super) fn fill<D: DrawTarget<Color = Rgb565>>(d: &mut D, r: Rectangle, c: Rgb565) -> Result<(), D::Error> {
    d.fill_solid(&r, c)
}

pub(super) fn hline<D: DrawTarget<Color = Rgb565>>(d: &mut D, y: i32, t: &Theme) -> Result<(), D::Error> {
    fill(d, rect(0, y, W - 1, y), t.rule)
}

pub(super) fn vline<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    x: i32,
    y0: i32,
    y1: i32,
    t: &Theme,
) -> Result<(), D::Error> {
    fill(d, rect(x, y0, x, y1), t.rule)
}

/// Fills the pixels of `area` inside a shape, its edges anti-aliased (4x4 samples a pixel) into `bg`.
pub(super) fn shape<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    area: Rectangle,
    c: Rgb565,
    bg: Rgb565,
    inside: impl Fn(f32, f32) -> bool,
) -> Result<(), D::Error> {
    let (x0, y0) = (area.top_left.x, area.top_left.y);
    let pixels = (0..area.size.height as i32)
        .flat_map(|y| (0..area.size.width as i32).map(move |x| (x0 + x, y0 + y)))
        .filter_map(|(x, y)| {
            let mut n = 0u8;
            for sy in 0..4 {
                for sx in 0..4 {
                    n += inside(
                        x as f32 + (sx as f32 + 0.5) / 4.0,
                        y as f32 + (sy as f32 + 0.5) / 4.0,
                    ) as u8;
                }
            }
            (n > 0).then(|| Pixel(Point::new(x, y), fonts::blend(bg, c, n * 15 / 16)))
        });
    d.draw_iter(pixels)
}

/// A disc of radius `r` centred on (`cx`, `cy`), in pixel coordinates (a pixel's centre at .5).
pub(super) fn disc<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    cx: f32,
    cy: f32,
    r: f32,
    c: Rgb565,
    bg: Rgb565,
) -> Result<(), D::Error> {
    let (cx, cy) = (cx + 0.5, cy + 0.5);
    // no floor() without std: the coordinates are positive, a cast truncates, one pixel more on each side
    let area = rect(
        (cx - r) as i32 - 1,
        (cy - r) as i32 - 1,
        (cx + r) as i32 + 1,
        (cy + r) as i32 + 1,
    );
    shape(d, area, c, bg, |x, y| {
        (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r
    })
}

/// A rectangle with round corners of radius `r`: the inside filled at once, the corners sampled.
pub(crate) fn rounded<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    area: Rectangle,
    r: i32,
    c: Rgb565,
    bg: Rgb565,
) -> Result<(), D::Error> {
    let (x0, y0) = (area.top_left.x, area.top_left.y);
    let (x1, y1) = (x0 + area.size.width as i32, y0 + area.size.height as i32);
    let r = r.min(area.size.height as i32 / 2).max(1);
    fill(d, rect(x0 + r, y0, x1 - r - 1, y1 - 1), c)?;
    fill(d, rect(x0, y0 + r, x1 - 1, y1 - r - 1), c)?;
    let rf = r as f32;
    for (left, upper) in [(true, true), (false, true), (true, false), (false, false)] {
        let (cx, cy) = (
            if left { x0 + r } else { x1 - r },
            if upper { y0 + r } else { y1 - r },
        );
        let corner = if left { (x0, cx - 1) } else { (cx, x1 - 1) };
        let rows = if upper { (y0, cy - 1) } else { (cy, y1 - 1) };
        let (fx, fy) = (cx as f32, cy as f32);
        shape(
            d,
            rect(corner.0, rows.0, corner.1, rows.1),
            c,
            bg,
            |x, y| (x - fx) * (x - fx) + (y - fy) * (y - fy) <= rf * rf,
        )?;
    }
    Ok(())
}
