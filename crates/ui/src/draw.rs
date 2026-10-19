//! Drawing of the screens: a top bar (top left key, clock, battery), the content, a bottom bar (the two bottom keys,
//! the ride page). Sizes and colours from the prototype (docs/prototype).

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Arc, Line, PrimitiveStyle, Rectangle, Triangle};
use u8g2_fonts::types::{FontColor, HorizontalAlignment as H, VerticalPosition as V};
use u8g2_fonts::{Content, Error, FontRenderer, fonts};

use crate::{Gnss, Hint, Icon, Kind, Screen, Ui, View};

const W: i32 = 240;
const H_: i32 = 320;
const TOP: i32 = 24;
const BOTTOM: i32 = 280;

/// Where the firmware draws the [`Screen::Status`] values.
pub const CONTENT: Rectangle = Rectangle::new(
    Point::new(0, TOP + 1),
    Size::new(W as u32, (BOTTOM - TOP - 1) as u32),
);

const HUGE: FontRenderer = FontRenderer::new::<fonts::u8g2_font_logisoso62_tn>();
const BIG: FontRenderer = FontRenderer::new::<fonts::u8g2_font_logisoso42_tn>();
const VALUE: FontRenderer = FontRenderer::new::<fonts::u8g2_font_logisoso28_tn>();
const TITLE: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB18_tf>();
const KEY: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB14_tf>();
const TEXT: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB12_tf>();
const LABEL: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB10_tf>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub bg: Rgb565,
    pub fg: Rgb565,
    pub mute: Rgb565,
    pub rule: Rgb565,
    pub chip: Rgb565,
    pub ok: Rgb565,
    pub danger: Rgb565,
    pub notice_bg: Rgb565,
    pub notice_fg: Rgb565,
    /// Background and text of the key labels by [`Kind`] (Go, Pause, Danger, Select).
    pub kinds: [(Rgb565, Rgb565); 4],
}

const fn rgb(hex: u32) -> Rgb565 {
    Rgb565::new(
        (hex >> 19) as u8 & 0x1f,
        (hex >> 10) as u8 & 0x3f,
        (hex >> 3) as u8 & 0x1f,
    )
}

const KINDS: [(Rgb565, Rgb565); 4] = [
    (rgb(0x237A35), rgb(0xFFFFFF)),
    (rgb(0xF59F00), rgb(0x111111)),
    (rgb(0xC92A2A), rgb(0xFFFFFF)),
    (rgb(0x1D4ED8), rgb(0xFFFFFF)),
];

pub const DAY: Theme = Theme {
    bg: rgb(0xFFFFFF),
    fg: rgb(0x111111),
    mute: rgb(0x555555),
    rule: rgb(0xDADADA),
    chip: rgb(0xECECEA),
    ok: rgb(0x237A35),
    danger: rgb(0xC92A2A),
    notice_bg: rgb(0x1A1A1D),
    notice_fg: rgb(0xFFFFFF),
    kinds: KINDS,
};

pub const NIGHT: Theme = Theme {
    bg: rgb(0x0B0B0C),
    fg: rgb(0xF2F2F2),
    mute: rgb(0xA3A3A3),
    rule: rgb(0x2C2C2C),
    chip: rgb(0x26262A),
    ok: rgb(0x69DB7C),
    danger: rgb(0xFF8787),
    notice_bg: rgb(0xF2F2F2),
    notice_fg: rgb(0x111111),
    kinds: KINDS,
};

/// Draws the whole screen; on [`Screen::Status`] the content is left blank for the firmware.
pub fn draw<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    d.clear(t.bg)?;
    match ui.screen() {
        Screen::Home => home(d, v, t)?,
        Screen::Status => {}
        Screen::Ride if ui.page() == 0 => ride_main(d, v, t)?,
        Screen::Ride => ride_altitude(d, v, t)?,
        Screen::Paused => banner(
            d,
            t,
            t.kinds[1],
            "En pause",
            "chrono arrêté",
            &[
                ("Distance", Val::Km(v.ride.distance_cm), " km"),
                ("Durée", Val::Duration(v.ride.timer_s), ""),
                ("Moyenne", Val::KmH(v.ride.avg_speed_mm_s()), " km/h"),
            ],
            "Éteindre le compteur\nenregistre la sortie.",
        )?,
        Screen::ConfirmStop => banner(
            d,
            t,
            t.kinds[2],
            "Terminer ?",
            "la sortie sera enregistrée",
            &[
                ("Distance", Val::Km(v.ride.distance_cm), " km"),
                ("Durée", Val::Duration(v.ride.timer_s), ""),
                ("D+", Val::Meters(Some(v.ride.ascent_m as i32)), " m"),
            ],
            "",
        )?,
        Screen::Saved => banner(
            d,
            t,
            t.kinds[0],
            "Enregistrée",
            "sortie terminée",
            &[
                ("Distance", Val::Km(v.ride.distance_cm), " km"),
                ("Durée", Val::Duration(v.ride.timer_s), ""),
                ("D+", Val::Meters(Some(v.ride.ascent_m as i32)), " m"),
                ("Moyenne", Val::KmH(v.ride.avg_speed_mm_s()), " km/h"),
            ],
            "",
        )?,
        Screen::PowerOff => banner(
            d,
            t,
            t.kinds[2],
            "Éteindre ?",
            if ui.power_off_saves_ride() {
                "la sortie sera enregistrée"
            } else {
                ""
            },
            &[],
            "Haut gauche : oui.\nUne autre touche : non.",
        )?,
    }
    if let Some(text) = ui.notice() {
        Rectangle::new(Point::new(0, BOTTOM - 44), Size::new(W as u32, 44))
            .into_styled(PrimitiveStyle::with_fill(t.notice_bg))
            .draw(d)?;
        text_at(
            d,
            &TEXT,
            text,
            Point::new(W / 2, BOTTOM - 22),
            V::Center,
            H::Center,
            t.notice_fg,
        )?;
    }
    let [k0, k1, k2] = ui.hints();
    top_bar(d, k0, v, t)?;
    bottom_bar(
        d,
        k1,
        k2,
        (ui.screen() == Screen::Ride).then_some(ui.page()),
        t,
    )
}

/// A value, formatted the French way (decimal comma); `-` when unknown.
#[derive(Clone, Copy)]
enum Val {
    Km(u32),
    KmH(Option<u32>),
    Duration(u32),
    Meters(Option<i32>),
    Bpm(Option<u8>),
    Clock(Option<(u8, u8)>),
}

impl Val {
    fn render<D: DrawTarget<Color = Rgb565>>(
        self,
        d: &mut D,
        font: &FontRenderer,
        at: Point,
        h: H,
        color: Rgb565,
    ) -> Result<(), D::Error> {
        let tenths = |x: u32| (x / 10, x % 10);
        let r = match self {
            Val::Km(cm) => {
                let (a, b) = tenths(cm / 10_000);
                text_at(d, font, format_args!("{a},{b}"), at, V::Center, h, color)
            }
            Val::KmH(Some(mm_s)) => {
                let (a, b) = tenths((mm_s * 36 + 500) / 1000);
                text_at(d, font, format_args!("{a},{b}"), at, V::Center, h, color)
            }
            Val::Duration(s) => text_at(
                d,
                font,
                format_args!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
                at,
                V::Center,
                h,
                color,
            ),
            Val::Meters(Some(m)) => text_at(d, font, format_args!("{m}"), at, V::Center, h, color),
            Val::Bpm(Some(b)) => text_at(d, font, format_args!("{b}"), at, V::Center, h, color),
            Val::Clock(Some((hh, mm))) => text_at(
                d,
                font,
                format_args!("{hh}:{mm:02}"),
                at,
                V::Center,
                h,
                color,
            ),
            Val::KmH(None) | Val::Meters(None) | Val::Bpm(None) | Val::Clock(None) => {
                text_at(d, font, "--", at, V::Center, h, color)
            }
        };
        r.map(drop)
    }
}

/// Page 1: speed, then time, distance, D+ and heart rate.
fn ride_main<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    big_cell(d, t, "VITESSE KM/H", Val::KmH(v.speed_mm_s), &HUGE)?;
    grid(
        d,
        t,
        [
            ("DURÉE", Val::Duration(v.ride.timer_s)),
            ("DISTANCE KM", Val::Km(v.ride.distance_cm)),
            ("D+ M", Val::Meters(Some(v.ride.ascent_m as i32))),
            ("FC BPM", Val::Bpm(v.heart_rate)),
        ],
    )
}

/// Page 2: altitude, then averages, clock and D-.
fn ride_altitude<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    big_cell(
        d,
        t,
        "ALTITUDE M",
        Val::Meters(v.altitude_dm.map(|a| a / 10)),
        &HUGE,
    )?;
    grid(
        d,
        t,
        [
            ("MOYENNE KM/H", Val::KmH(v.ride.avg_speed_mm_s())),
            ("MAX KM/H", Val::KmH(Some(v.ride.max_speed_mm_s))),
            ("HEURE", Val::Clock(v.clock)),
            ("D- M", Val::Meters(Some(v.ride.descent_m as i32))),
        ],
    )
}

const BIG_CELL: i32 = 108;

fn big_cell<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    label: &str,
    value: Val,
    font: &FontRenderer,
) -> Result<(), D::Error> {
    text_at(
        d,
        &LABEL,
        label,
        Point::new(W / 2, TOP + 14),
        V::Center,
        H::Center,
        t.mute,
    )?;
    value.render(d, font, Point::new(W / 2, TOP + 64), H::Center, t.fg)?;
    hline(d, TOP + BIG_CELL, t)
}

/// Four values in two rows below the big cell.
fn grid<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    cells: [(&str, Val); 4],
) -> Result<(), D::Error> {
    let top = TOP + BIG_CELL;
    let h = (BOTTOM - top) / 2;
    hline(d, top + h, t)?;
    Line::new(Point::new(W / 2, top), Point::new(W / 2, BOTTOM))
        .into_styled(PrimitiveStyle::with_stroke(t.rule, 1))
        .draw(d)?;
    for (i, (label, value)) in cells.into_iter().enumerate() {
        let x = W / 4 + (i as i32 % 2) * W / 2;
        let y = top + (i as i32 / 2) * h;
        text_at(
            d,
            &LABEL,
            label,
            Point::new(x, y + 14),
            V::Center,
            H::Center,
            t.mute,
        )?;
        value.render(d, &VALUE, Point::new(x, y + 46), H::Center, t.fg)?;
    }
    Ok(())
}

fn home<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    Val::Clock(v.clock).render(d, &BIG, Point::new(W / 2, TOP + 44), H::Center, t.fg)?;
    hline(d, TOP + 84, t)?;
    let gnss = match v.gnss {
        Gnss::Off => (t.mute, Line2::Str("éteint")),
        Gnss::Searching { in_view } => (t.danger, Line2::Searching(in_view)),
        Gnss::Fix { used } => (t.ok, Line2::Fix(used)),
    };
    let y0 = TOP + 84 + 24;
    row(d, t, y0, "GPS", gnss.0, gnss.1)?;
    row(
        d,
        t,
        y0 + 30,
        "Altitude",
        t.fg,
        Line2::Meters(v.altitude_dm.map(|a| a / 10)),
    )?;
    row(d, t, y0 + 60, "Sorties", t.fg, Line2::Count(v.rides_stored))?;
    row(d, t, y0 + 90, "Place", t.fg, Line2::Hours(v.free_hours))?;
    row(
        d,
        t,
        y0 + 120,
        "Batterie",
        t.fg,
        Line2::Percent(v.battery_pct),
    )
}

/// The right side of a [`row`].
#[derive(Clone, Copy)]
enum Line2 {
    Str(&'static str),
    Searching(u8),
    Fix(u8),
    Meters(Option<i32>),
    Count(Option<u16>),
    Hours(Option<u32>),
    Percent(Option<u8>),
}

fn row<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    y: i32,
    label: &str,
    color: Rgb565,
    value: Line2,
) -> Result<(), D::Error> {
    text_at(
        d,
        &KEY,
        label,
        Point::new(12, y),
        V::Center,
        H::Left,
        t.mute,
    )?;
    let at = Point::new(W - 12, y);
    let r = |d: &mut D, args: core::fmt::Arguments<'_>| {
        text_at(d, &KEY, args, at, V::Center, H::Right, color)
    };
    let res = match value {
        Line2::Str(s) => r(d, format_args!("{s}")),
        Line2::Searching(n) => r(d, format_args!("recherche, {n} vus")),
        Line2::Fix(n) => r(d, format_args!("fix, {n} sat")),
        Line2::Meters(Some(m)) => r(d, format_args!("{m} m")),
        Line2::Count(Some(n)) => r(d, format_args!("{n}")),
        Line2::Hours(Some(h)) => r(d, format_args!("~{h} h")),
        Line2::Percent(Some(p)) => r(d, format_args!("{p} %")),
        Line2::Meters(None) | Line2::Count(None) | Line2::Hours(None) | Line2::Percent(None) => {
            r(d, format_args!("-"))
        }
    };
    res.map(drop)
}

/// A coloured band with a title, then a few values and a note.
#[allow(clippy::too_many_arguments)]
fn banner<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    (bg, fg): (Rgb565, Rgb565),
    title: &str,
    sub: &str,
    rows: &[(&str, Val, &str)],
    note: &str,
) -> Result<(), D::Error> {
    Rectangle::new(Point::new(0, TOP + 1), Size::new(W as u32, 64))
        .into_styled(PrimitiveStyle::with_fill(bg))
        .draw(d)?;
    text_at(
        d,
        &TITLE,
        title,
        Point::new(12, TOP + 24),
        V::Center,
        H::Left,
        fg,
    )?;
    if !sub.is_empty() {
        text_at(
            d,
            &TEXT,
            sub,
            Point::new(12, TOP + 48),
            V::Center,
            H::Left,
            fg,
        )?;
    }
    let mut y = TOP + 65 + 26;
    for (label, value, unit) in rows {
        text_at(
            d,
            &KEY,
            *label,
            Point::new(12, y),
            V::Center,
            H::Left,
            t.mute,
        )?;
        let unit_w = KEY
            .get_rendered_dimensions(*unit, Point::zero(), V::Center)
            .map_or(0, |r| r.advance.x);
        if !unit.is_empty() {
            text_at(
                d,
                &KEY,
                *unit,
                Point::new(W - 12, y),
                V::Center,
                H::Right,
                t.mute,
            )?;
        }
        value.render(d, &KEY, Point::new(W - 12 - unit_w, y), H::Right, t.fg)?;
        y += 30;
    }
    if !note.is_empty() {
        text_at(
            d,
            &TEXT,
            note,
            Point::new(12, BOTTOM - 44),
            V::Top,
            H::Left,
            t.mute,
        )?;
    }
    Ok(())
}

fn top_bar<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    hint: Option<Hint>,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    if let Some(h) = hint {
        let w = hint_width(&h) + 2 * PAD;
        key_label(
            d,
            &h,
            Rectangle::new(Point::zero(), Size::new(w as u32, TOP as u32)),
            false,
            t,
        )?;
    }
    let x = W - 8;
    if let Some(p) = v.battery_pct {
        text_at(
            d,
            &LABEL,
            format_args!("{p} %"),
            Point::new(x, TOP / 2),
            V::Center,
            H::Right,
            t.mute,
        )?;
    }
    if let Some((h, m)) = v.clock {
        text_at(
            d,
            &LABEL,
            format_args!("{h}:{m:02}"),
            Point::new(x - 42, TOP / 2),
            V::Center,
            H::Right,
            t.mute,
        )?;
    }
    hline(d, TOP, t)
}

fn bottom_bar<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    left: Option<Hint>,
    right: Option<Hint>,
    page: Option<u8>,
    t: &Theme,
) -> Result<(), D::Error> {
    let h = (H_ - BOTTOM) as u32;
    let half = Size::new(W as u32 / 2 - 8, h);
    if let Some(l) = left {
        key_label(d, &l, Rectangle::new(Point::new(0, BOTTOM), half), false, t)?;
    }
    if let Some(r) = right {
        key_label(
            d,
            &r,
            Rectangle::new(Point::new(W / 2 + 8, BOTTOM), half),
            true,
            t,
        )?;
    }
    if let Some(p) = page {
        for i in 0..crate::PAGES {
            let x = W / 2 - 4 * (crate::PAGES as i32 - 1) + 8 * i as i32;
            let style = if i == p {
                PrimitiveStyle::with_fill(t.fg)
            } else {
                PrimitiveStyle::with_stroke(t.mute, 1)
            };
            embedded_graphics::primitives::Circle::with_center(
                Point::new(x, BOTTOM + h as i32 / 2),
                6,
            )
            .into_styled(style)
            .draw(d)?;
        }
    }
    hline(d, BOTTOM, t)
}

const ICON: i32 = 12;
const GAP: i32 = 6;
/// Space between a key label and the edge of its box.
const PAD: i32 = 8;

fn hint_width(h: &Hint) -> i32 {
    let text = KEY
        .get_rendered_dimensions(h.text, Point::zero(), V::Center)
        .map_or(0, |r| r.advance.x);
    text + (h.before.is_some() as i32 + h.after.is_some() as i32) * (ICON + GAP)
}

/// A key label in its box, against the key side of the box.
fn key_label<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    h: &Hint,
    area: Rectangle,
    right: bool,
    t: &Theme,
) -> Result<(), D::Error> {
    let (bg, fg) = match h.kind {
        Kind::Plain => (t.chip, t.fg),
        Kind::Go => t.kinds[0],
        Kind::Pause => t.kinds[1],
        Kind::Danger => t.kinds[2],
        Kind::Select => t.kinds[3],
    };
    area.into_styled(PrimitiveStyle::with_fill(bg)).draw(d)?;
    let width = hint_width(h);
    debug_assert!(
        width + 2 * PAD <= area.size.width as i32,
        "key label {:?} too wide: {width} + 2 * {PAD} > {}",
        h.text,
        area.size.width
    );
    let y = area.center().y;
    let mut x = if right {
        area.top_left.x + area.size.width as i32 - PAD - width
    } else {
        area.top_left.x + 10
    };
    if let Some(i) = h.before {
        icon(d, i, Point::new(x, y), fg)?;
        x += ICON + GAP;
    }
    let r = text_at(d, &KEY, h.text, Point::new(x, y), V::Center, H::Left, fg)?;
    if let Some(i) = h.after {
        let end = r.map_or(x, |r| r.top_left.x + r.size.width as i32);
        icon(d, i, Point::new(end + GAP, y), fg)?;
    }
    Ok(())
}

/// An [`ICON`]-wide icon, its left side at `at.x`, centred on `at.y`.
fn icon<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    i: Icon,
    at: Point,
    c: Rgb565,
) -> Result<(), D::Error> {
    let fill = PrimitiveStyle::with_fill(c);
    let (x, y, s) = (at.x, at.y, ICON);
    match i {
        Icon::Play | Icon::Right => Triangle::new(
            Point::new(x, y - s / 2),
            Point::new(x + s - 1, y),
            Point::new(x, y + s / 2),
        )
        .into_styled(fill)
        .draw(d),
        Icon::Left => Triangle::new(
            Point::new(x + s - 1, y - s / 2),
            Point::new(x, y),
            Point::new(x + s - 1, y + s / 2),
        )
        .into_styled(fill)
        .draw(d),
        Icon::Pause => {
            for dx in [1, s - 5] {
                Rectangle::new(Point::new(x + dx, y - s / 2), Size::new(4, s as u32))
                    .into_styled(fill)
                    .draw(d)?;
            }
            Ok(())
        }
        Icon::Stop => Rectangle::new(
            Point::new(x + 1, y - s / 2 + 1),
            Size::new(s as u32 - 2, s as u32 - 2),
        )
        .into_styled(fill)
        .draw(d),
        Icon::Power => {
            let stroke = PrimitiveStyle::with_stroke(c, 2);
            Arc::new(
                Point::new(x, y - s / 2 + 1),
                s as u32,
                (-60.0).deg(),
                300.0.deg(),
            )
            .into_styled(stroke)
            .draw(d)?;
            Line::new(
                Point::new(x + s / 2, y - s / 2 - 1),
                Point::new(x + s / 2, y),
            )
            .into_styled(stroke)
            .draw(d)
        }
    }
}

fn hline<D: DrawTarget<Color = Rgb565>>(d: &mut D, y: i32, t: &Theme) -> Result<(), D::Error> {
    Line::new(Point::new(0, y), Point::new(W - 1, y))
        .into_styled(PrimitiveStyle::with_stroke(t.rule, 1))
        .draw(d)
}

/// Text through u8g2-fonts; a glyph missing from the font is a bug (debug builds stop), skipped in release.
fn text_at<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    font: &FontRenderer,
    content: impl Content,
    at: Point,
    v: V,
    h: H,
    color: Rgb565,
) -> Result<Option<Rectangle>, D::Error> {
    match font.render_aligned(content, at, v, h, FontColor::Transparent(color), d) {
        Ok(r) => {
            debug_assert!(
                r.is_none_or(|r| r.top_left.x >= 0 && r.top_left.x + r.size.width as i32 <= W),
                "text out of the screen: {r:?}"
            );
            Ok(r)
        }
        Err(Error::DisplayError(e)) => Err(e),
        Err(Error::GlyphNotFound(c)) => {
            debug_assert!(false, "glyph {c:?} not in the font");
            Ok(None)
        }
        Err(Error::BackgroundColorNotSupported) => Ok(None),
    }
}
