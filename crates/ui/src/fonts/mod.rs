//! Anti-aliased bitmap fonts: Barlow Condensed rasterized by tools/build_fonts.py (SIL OFL 1.1, see
//! crates/ui/fonts/OFL.txt), drawn by blending each pixel between the text colour and the background colour.
//!
//! Text is placed by its ink, not its advance box: [`Align`] lines up the first or last inked pixel (or the middle of
//! the ink), and a line is centred vertically on the middle of the capitals. The icons are glyphs of every font (see
//! [`icon`]), so they line up with the text they are written with.

#[rustfmt::skip]
mod generated;

use core::fmt::{self, Write};

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

pub use generated::*;

/// Icons, as characters of every font (private use area, in the order of tools/build_fonts.py).
pub mod icon {
    pub const PLAY: char = '\u{e000}';
    pub const PAUSE: char = '\u{e001}';
    pub const STOP: char = '\u{e002}';
    pub const LEFT: char = '\u{e003}';
    pub const RIGHT: char = '\u{e004}';
    pub const POWER: char = '\u{e005}';
    pub const CHECK: char = '\u{e006}';
    pub const CROSS: char = '\u{e007}';
    pub const DOT: char = '\u{e008}';
    pub const MENU: char = '\u{e009}';
    pub const DOWN: char = '\u{e00a}';
    pub const CHEVRON: char = '\u{e00b}';
    pub const CYCLE: char = '\u{e00c}';
    pub const UNDO: char = '\u{e00d}';
    pub const RING: char = '\u{e00e}';
    pub const ZOOM: char = '\u{e00f}';
    pub const OFF_ROUTE: char = '\u{e010}';
}

pub struct Font {
    pub px: u8,
    pub ascent: i16,
    pub descent: i16,
    /// Height of the capitals (and of the figures and icons), pixels above the baseline.
    pub cap: i16,
    glyphs: &'static [Glyph],
    /// (left, right, adjustment in 1/64 px), sorted.
    kerning: &'static [(char, char, i16)],
    /// 4-bit alpha, two pixels a byte (high nibble first), each glyph from `off`, rows packed.
    bitmaps: &'static [u8],
}

struct Glyph {
    c: char,
    /// 1/64 px.
    adv: u16,
    /// Left of the bitmap from the pen, top of the bitmap above the baseline.
    x: i16,
    y: i16,
    w: u8,
    h: u8,
    off: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// How a text looks: font, colours, extra space between letters (1/64 px, like the prototype's `letter-spacing`).
#[derive(Clone, Copy)]
pub struct Style {
    pub font: &'static Font,
    pub color: Rgb565,
    /// Colour under the text: the glyph edges are blended into it.
    pub bg: Rgb565,
    pub tracking: i16,
}

impl Style {
    pub const fn new(font: &'static Font, color: Rgb565, bg: Rgb565) -> Self {
        Self {
            font,
            color,
            bg,
            tracking: 0,
        }
    }

    /// Letter spacing in thousandths of the font size (the prototype's labels: 60 = 0.06em).
    pub const fn spaced(mut self, per_mille: i16) -> Self {
        self.tracking = (self.font.px as i32 * 64 * per_mille as i32 / 1000) as i16;
        self
    }

    pub const fn on(mut self, bg: Rgb565) -> Self {
        self.bg = bg;
        self
    }

    pub const fn colored(mut self, color: Rgb565) -> Self {
        self.color = color;
        self
    }
}

impl Font {
    fn glyph(&self, c: char) -> Option<&Glyph> {
        let g = self
            .glyphs
            .binary_search_by_key(&c, |g| g.c)
            .ok()
            .map(|i| &self.glyphs[i]);
        debug_assert!(g.is_some(), "{c:?} not in the {} px font", self.px);
        g
    }

    fn kern(&self, a: char, b: char) -> i32 {
        self.kerning
            .binary_search_by(|&(x, y, _)| (x, y).cmp(&(a, b)))
            .map_or(0, |i| self.kerning[i].2 as i32)
    }

    /// Calls `f` with each glyph and its pen position (1/64 px from the start).
    fn layout(&self, text: &str, tracking: i16, mut f: impl FnMut(&Glyph, i32)) {
        let mut pen = 0;
        let mut prev = None;
        for c in text.chars() {
            let Some(g) = self.glyph(c) else { continue };
            if let Some(p) = prev {
                pen += self.kern(p, c) + tracking as i32;
            }
            f(g, pen);
            pen += g.adv as i32;
            prev = Some(c);
        }
    }

    /// First and last inked columns + 1, from the start of the text; (0, 0) without ink.
    fn ink(&self, text: &str, tracking: i16) -> (i32, i32) {
        let mut ink: Option<(i32, i32)> = None;
        self.layout(text, tracking, |g, pen| {
            if g.w > 0 {
                let x = round64(pen) + g.x as i32;
                let (l, r) = ink.unwrap_or((x, x + g.w as i32));
                ink = Some((l.min(x), r.max(x + g.w as i32)));
            }
        });
        ink.unwrap_or((0, 0))
    }

    /// Advance of `text`: where the pen ends, in pixels.
    pub fn advance(&self, text: &str, tracking: i16) -> i32 {
        let mut end = 0;
        self.layout(text, tracking, |g, pen| end = pen + g.adv as i32);
        round64(end)
    }

    /// Width of the ink of `text`.
    pub fn width(&self, text: &str, tracking: i16) -> i32 {
        let (l, r) = self.ink(text, tracking);
        r - l
    }
}

fn round64(v: i32) -> i32 {
    (v + 32).div_euclid(64)
}

/// A line of text up to 64 bytes, for formatted values.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Line {
    buf: [u8; 64],
    len: usize,
}

impl Line {
    pub fn new(args: fmt::Arguments<'_>) -> Self {
        let mut l = Self {
            buf: [0; 64],
            len: 0,
        };
        let _ = l.write_fmt(args);
        l
    }

    pub fn as_str(&self) -> &str {
        // only whole `str`s are written
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl fmt::Debug for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len();
        if self.len + n > self.buf.len() {
            debug_assert!(false, "line too long: {s:?}");
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + n].copy_from_slice(s.as_bytes());
        self.len += n;
        Ok(())
    }
}

/// Draws one line of text, its ink aligned on `x` and its capitals centred on `cy`; returns the ink box (the
/// capitals' height).
pub fn text<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Style,
    text: &str,
    x: i32,
    cy: i32,
    align: Align,
) -> Result<Rectangle, D::Error> {
    let f = s.font;
    let (l, r) = f.ink(text, s.tracking);
    let left = match align {
        Align::Left => x,
        Align::Center => x - (r - l) / 2,
        Align::Right => x - (r - l),
    };
    let origin = left - l;
    let baseline = cy + (f.cap as i32 + 1) / 2;
    draw_at(d, s, text, origin, baseline)?;
    debug_assert!(
        left >= 0 && left + (r - l) <= d.bounding_box().size.width as i32,
        "{text:?} out of the screen ({left}..{})",
        left + (r - l)
    );
    Ok(Rectangle::new(
        Point::new(left, baseline - f.cap as i32),
        Size::new((r - l) as u32, f.cap as u32),
    ))
}

/// Draws `text` with its pen starting at `origin` on `baseline`.
fn draw_at<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Style,
    text: &str,
    origin: i32,
    baseline: i32,
) -> Result<(), D::Error> {
    let f = s.font;
    let mut res = Ok(());
    f.layout(text, s.tracking, |g, pen| {
        if res.is_err() || g.w == 0 {
            return;
        }
        let gx = origin + round64(pen) + g.x as i32;
        let gy = baseline - g.y as i32;
        let start = g.off as usize * 2;
        let pixels = (0..g.h as i32)
            .flat_map(move |row| (0..g.w as i32).map(move |col| (row, col)))
            .filter_map(|(row, col)| {
                let i = start + (row * g.w as i32 + col) as usize;
                let byte = f.bitmaps[i / 2];
                let a = if i.is_multiple_of(2) {
                    byte >> 4
                } else {
                    byte & 0x0f
                };
                (a > 0).then(|| Pixel(Point::new(gx + col, gy + row), blend(s.bg, s.color, a)))
            });
        res = d.draw_iter(pixels);
    });
    res
}

/// Draws a value that changes while it is shown, so that it does not move when it changes: placed by its advance
/// (the figures are tabular), right-aligned in a slot that `align` places on `x` like [`text`] would place the
/// slot's template. The template is `text` with each figure made a `0`, at least `min` (`"00,0"` keeps the comma of
/// a speed in place from 9,8 to 32,4). A text without figures (an unknown value) is placed as a text.
pub fn value<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Style,
    text: &str,
    min: &str,
    x: i32,
    cy: i32,
    align: Align,
) -> Result<Rectangle, D::Error> {
    if !text.chars().any(|c| c.is_ascii_digit()) {
        return self::text(d, s, text, x, cy, align);
    }
    let f = s.font;
    let origin = value_origin(f, s.tracking, text, min, x, align);
    let baseline = cy + (f.cap as i32 + 1) / 2;
    draw_at(d, s, text, origin, baseline)?;
    let (tl, tr) = f.ink(text, s.tracking);
    debug_assert!(
        origin + tl >= 0 && origin + tr <= d.bounding_box().size.width as i32,
        "{text:?} out of the screen"
    );
    Ok(Rectangle::new(
        Point::new(origin + tl, baseline - f.cap as i32),
        Size::new((tr - tl) as u32, f.cap as u32),
    ))
}

/// Where the pen starts for [`value`].
fn value_origin(f: &Font, tracking: i16, text: &str, min: &str, x: i32, align: Align) -> i32 {
    let zeros = Line::new(format_args!("{}", Zeros(text)));
    let template = if zeros.as_str().chars().count() >= min.chars().count() {
        zeros.as_str()
    } else {
        min
    };
    let (l, r) = f.ink(template, tracking);
    let template_origin = match align {
        Align::Left => x - l,
        Align::Center => x - (r - l) / 2 - l,
        Align::Right => x - r,
    };
    template_origin + f.advance(template, tracking) - f.advance(text, tracking)
}

/// `text` with each figure written `0`.
struct Zeros<'a>(&'a str);

impl fmt::Display for Zeros<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            f.write_char(if c.is_ascii_digit() { '0' } else { c })?;
        }
        Ok(())
    }
}

/// Several lines of text, `line` pixels apart, the first one centred on `cy`.
pub fn lines<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Style,
    text: &str,
    x: i32,
    cy: i32,
    line: i32,
    align: Align,
) -> Result<(), D::Error> {
    for (i, l) in text.lines().enumerate() {
        self::text(d, s, l, x, cy + i as i32 * line, align)?;
    }
    Ok(())
}

/// `bg` to `fg` by `a` / 15, per channel.
pub fn blend(bg: Rgb565, fg: Rgb565, a: u8) -> Rgb565 {
    if a >= 15 {
        return fg;
    }
    let mix = |b: u8, f: u8| ((b as u16 * (15 - a as u16) + f as u16 * a as u16 + 7) / 15) as u8;
    Rgb565::new(
        mix(bg.r(), fg.r()),
        mix(bg.g(), fg.g()),
        mix(bg.b(), fg.b()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted() {
        for f in [
            &HUGE,
            &ALT,
            &VALUE,
            &TITLE,
            &STAT,
            &HEADING,
            &HEADING_SMALL,
            &MENU,
            &BANNER,
            &KEY,
            &BODY_BOLD,
            &ROW,
            &ROW_SMALL,
            &BODY,
            &NOTE_LARGE,
            &SUB,
            &NOTE,
            &NOTE_BOLD,
            &BAR,
            &LABEL,
            &LABEL_SMALL,
            &LABEL_TINY,
            &MARK,
        ] {
            assert!(f.glyphs.windows(2).all(|w| w[0].c < w[1].c), "{} px", f.px);
            assert!(
                f.kerning
                    .windows(2)
                    .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)),
                "{} px",
                f.px
            );
            for g in f.glyphs {
                assert!(
                    g.off as usize + (g.w as usize * g.h as usize).div_ceil(2) <= f.bitmaps.len()
                );
            }
        }
    }

    #[test]
    fn figures_are_tabular() {
        let w: [u16; 10] =
            core::array::from_fn(|i| VALUE.glyph(char::from(b'0' + i as u8)).unwrap().adv);
        assert!(w.iter().all(|&x| x == w[0]));
    }

    #[test]
    fn kerning_applies() {
        // Barlow kerns "AV"
        assert!(TITLE.kern('A', 'V') < 0);
        assert_eq!(TITLE.kern('0', '0'), 0);
    }

    /// Pen position of the `n`-th character of a value placed by [`value`].
    fn pen_at(f: &Font, text: &str, min: &str, align: Align, n: usize) -> i32 {
        let end: String = text.chars().take(n).collect();
        value_origin(f, 0, text, min, 120, align) + f.advance(&end, 0)
    }

    extern crate std;
    use std::string::String;

    #[test]
    fn changing_values_keep_their_place() {
        // the comma of a speed, whatever the figures and from one to two figures
        let comma = |t: &str| pen_at(&HUGE, t, "00,0", Align::Center, t.find(',').unwrap());
        for t in ["9,8", "32,4", "18,1", "11,1", "0,0", "47,7"] {
            assert_eq!(comma(t), comma("32,4"), "{t}");
        }
        // the colons of a duration
        let colon = |t: &str| pen_at(&VALUE, t, "", Align::Center, t.rfind(':').unwrap());
        for t in ["1:42:08", "1:11:11", "0:00:00", "9:59:59"] {
            assert_eq!(colon(t), colon("1:42:08"), "{t}");
        }
        // a right-aligned value keeps its right edge
        let end = |t: &str| pen_at(&TITLE, t, "", Align::Right, t.chars().count());
        assert_eq!(end("1:42:08"), end("1:11:11"));
        assert_eq!(end("48,6 km"), end("11,1 km"));
    }

    #[test]
    fn blend_ends() {
        let (b, f) = (Rgb565::WHITE, Rgb565::BLACK);
        assert_eq!(blend(b, f, 15), f);
        assert_eq!(blend(b, f, 0), b);
    }
}
