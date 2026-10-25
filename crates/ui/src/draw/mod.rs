//! Drawing of the screens: a top bar (top left key, clock, battery), the content, a bottom bar in three fixed
//! columns (bottom left key, ride page, bottom right key). Fonts, sizes, colours and positions from the prototype
//! (docs/prototype), measured with tools/prototype_shots.py; a few alignments fixed on the way (values by their
//! advance, label and value of a row on one baseline, icons on the capitals).
//!
//! Vertical positions are the centres of the capitals ([`fonts::text`]), horizontal ones the edges of the ink.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle, RoundedRectangle, Triangle};

use crate::fonts::{self, Align, Font, Line as Fmt, Style, icon};
use crate::{BannerKind, Gnss, Hint, Item, Kind, Link, Page, RideCard, Role, Screen, Ui, View};

mod chrome;
mod primitives;
mod ride;
mod screens;

pub(crate) use primitives::rounded;

pub(crate) const W: i32 = 240;
const H: i32 = 320;
/// Rule under the top bar, rule over the key bar.
pub(crate) const TOP: i32 = 30;
pub(crate) const BOTTOM: i32 = 279;
/// Side margin of the content.
const MARGIN: i32 = 12;
/// Side margin of the key labels, in their boxes.
const KEY_PAD: i32 = 8;
/// The blue mark of a key box, on the side of its key.
const KEY_MARK: i32 = 5;
/// Width of the top key box: the middle of the top bar stays free for the clock.
const TOP_KEY: i32 = 100;
/// Right margin of the top bar's battery.
const BAR_PAD: i32 = 4;
/// Columns of the key bar: left key, page, right key.
const BAR_LEFT: i32 = 104;
const BAR_RIGHT: i32 = 136;

/// Where the firmware draws the [`Screen::Status`] values.
pub const CONTENT: Rectangle = Rectangle::new(
    Point::new(0, TOP + 1),
    Size::new(W as u32, (BOTTOM - TOP - 1) as u32),
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub bg: Rgb565,
    pub fg: Rgb565,
    pub mute: Rgb565,
    pub rule: Rgb565,
    pub chip: Rgb565,
    pub ok: Rgb565,
    pub warn: Rgb565,
    pub danger: Rgb565,
    /// Selected menu entry, confirmation.
    pub sel: Rgb565,
    /// The colour of the physical keys, on the key boxes.
    pub key_mark: Rgb565,
    pub banner_bg: Rgb565,
    pub banner_fg: Rgb565,
    pub info: Rgb565,
    pub attention: Rgb565,
    /// Area under an altitude profile, and the rider's point on it.
    pub profile: Rgb565,
    pub me: Rgb565,
    /// The next climb on the climbs page.
    pub next: Rgb565,
    /// The route on the map, ahead and done.
    pub route: Rgb565,
    pub done: Rgb565,
    /// The background map, by class ([`route::map::class`]).
    pub map: [Rgb565; route::map::class::COUNT],
    /// Background and text of the key labels by [`Kind`] (Go, Pause, Danger, Select).
    pub kinds: [(Rgb565, Rgb565); 4],
}

const fn rgb(hex: u32) -> Rgb565 {
    // rounded to the nearest 565 level
    const fn c(v: u32, bits: u32) -> u8 {
        ((v * ((1 << bits) - 1) + 127) / 255) as u8
    }
    Rgb565::new(
        c(hex >> 16 & 0xff, 5),
        c(hex >> 8 & 0xff, 6),
        c(hex & 0xff, 5),
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
    warn: rgb(0xB35C00),
    danger: rgb(0xC92A2A),
    sel: rgb(0x1D4ED8),
    key_mark: rgb(0x3BB3E0),
    banner_bg: rgb(0x1A1A1D),
    banner_fg: rgb(0xFFFFFF),
    info: rgb(0x4C8DFF),
    attention: rgb(0xF59F00),
    profile: rgb(0xD6D6D2),
    me: rgb(0x1D4ED8),
    next: rgb(0xFFF4E0),
    route: rgb(0xD9480F),
    done: rgb(0x9A9A9A),
    // forest, water, residential, contour, major contour, stream, river, path, track, service, minor, major
    map: [
        rgb(0xDDEBD3), rgb(0xC6DDF2), rgb(0xEEEDEA), rgb(0xE3CDB8), rgb(0xC9A585), rgb(0x9CC3E6),
        rgb(0x7FB0DE), rgb(0x9E9E9E), rgb(0x8F8676), rgb(0xBDBDBD), rgb(0x9C9C9C), rgb(0x7A7A7A),
    ],
    kinds: KINDS,
};

pub const NIGHT: Theme = Theme {
    bg: rgb(0x0B0B0C),
    fg: rgb(0xF2F2F2),
    mute: rgb(0xA3A3A3),
    rule: rgb(0x2C2C2C),
    chip: rgb(0x26262A),
    ok: rgb(0x69DB7C),
    warn: rgb(0xFFA94D),
    danger: rgb(0xFF8787),
    sel: rgb(0x1D4ED8),
    key_mark: rgb(0x3BB3E0),
    banner_bg: rgb(0xF2F2F2),
    banner_fg: rgb(0x111111),
    info: rgb(0x4C8DFF),
    attention: rgb(0xF59F00),
    profile: rgb(0x3A3A3E),
    me: rgb(0x6EA8FF),
    next: rgb(0x1E1A12),
    route: rgb(0xFF7A45),
    done: rgb(0x5A5A5E),
    map: [
        rgb(0x14261A), rgb(0x13283F), rgb(0x1C1C1E), rgb(0x3A2F25), rgb(0x5A4636), rgb(0x285A8A),
        rgb(0x2F6DA8), rgb(0x666666), rgb(0x7D735F), rgb(0x4A4A4A), rgb(0x727272), rgb(0x9A9A9A),
    ],
    kinds: KINDS,
};

impl Theme {
    fn style(&self, font: &'static Font) -> Style {
        Style::new(font, self.fg, self.bg)
    }

    fn mute(&self, font: &'static Font) -> Style {
        Style::new(font, self.mute, self.bg)
    }

    /// The prototype's small uppercase labels (letter spacing 0.06em).
    fn label(&self, font: &'static Font) -> Style {
        self.mute(font).spaced(60)
    }

    fn kind(&self, kind: Kind) -> (Rgb565, Rgb565) {
        match kind {
            Kind::Plain => (self.chip, self.fg),
            Kind::Nothing => (self.chip, self.mute),
            Kind::Go => self.kinds[0],
            Kind::Pause => self.kinds[1],
            Kind::Danger => self.kinds[2],
            Kind::Select => self.kinds[3],
        }
    }

    /// Colour of a gradient: orange from 3 %, red from 6 %, dark red from 9 %.
    fn grade(&self, pct: i8) -> Rgb565 {
        match pct {
            ..3 => self.fg,
            3..6 => rgb(0xF08C00),
            6..9 => rgb(0xD9302C),
            9.. => rgb(0x7D1A1A),
        }
    }

    /// Colour of a stretch of a climb by its gradient, and of the text on it: grey under 1 %, yellow under 3 %, then
    /// as [`Theme::grade`].
    fn slope(&self, pct: i32) -> (Rgb565, Rgb565) {
        let dark = rgb(0x111111);
        match pct {
            ..1 => (rgb(0xBFC5CC), dark),
            1..3 => (rgb(0xF2C94C), dark),
            3..6 => (rgb(0xF08C00), dark),
            6..9 => (rgb(0xD9302C), rgb(0xFFFFFF)),
            9.. => (rgb(0x7D1A1A), rgb(0xFFFFFF)),
        }
    }
}

/// Draws the whole screen; on [`Screen::Status`] the content is left blank for the firmware. `route`: the route
/// loaded (the map page draws it).
pub fn draw<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    v: &View,
    route: Option<&route::Route<'_>>,
    map: Option<&route::map::Map<'_>>,
    t: &Theme,
) -> Result<(), D::Error> {
    d.clear(t.bg)?;
    // the charging screen and the splash are all custom: no bars, no banner
    match ui.screen() {
        Screen::Charging => return screens::charging(d, ui, v, t),
        Screen::Splash => return screens::splash(d, ui, t),
        _ => {}
    }
    match ui.screen() {
        Screen::Home => screens::home(d, ui, v, t)?,
        Screen::Menu => screens::menu(d, ui, v, t, None, screens::MENU_NOTE)?,
        Screen::PauseMenu => screens::menu(d, ui, v, t, Some("SORTIE EN PAUSE"), screens::PAUSE_MENU_NOTE)?,
        Screen::Status => {}
        Screen::Sensors => screens::sensors(d, ui, v, t)?,
        Screen::Charging | Screen::Splash => {}
        Screen::SensorScan => screens::sensor_scan(d, ui, v, t)?,
        Screen::Calibrate => screens::calibrate(d, v, t)?,
        Screen::Ride => match ui.page() {
            Some(Page::Altitude) => ride::ride_altitude(d, v, t)?,
            Some(Page::Climbs) => ride::ride_climbs(d, v, route, t)?,
            Some(Page::Map) => crate::map::draw(d, ui.zoom_m(), v, route, map, t)?,
            Some(Page::Effort) => ride::ride_effort(d, ui, v, t)?,
            Some(Page::Cardio) => ride::ride_cardio(d, ui, v, t)?,
            Some(Page::Essential) | None => ride::ride_essential(d, ui, v, t)?,
        },
        Screen::Paused => screens::paused(d, v, t, "En pause", "chrono arrêté", screens::PAUSED_NOTE)?,
        Screen::Resume => screens::paused(
            d,
            v,
            t,
            "Sortie en pause",
            "le compteur était éteint",
            screens::RESUME_NOTE,
        )?,
        Screen::ConfirmStop => screens::confirm_stop(d, v, t)?,
        Screen::ConfirmEmpty => screens::confirm_empty(d, v, t)?,
        Screen::ConfirmDiscard => screens::confirm_discard(d, v, t)?,
        Screen::ConfirmDiscardAgain => screens::confirm_discard_again(d, v, t)?,
        Screen::Saved => screens::saved(d, v, t)?,
        Screen::Rides => screens::rides(d, ui, v, t)?,
        Screen::ConfirmDelete => screens::confirm_delete(d, v, t)?,
        Screen::About => screens::about(d, ui, t)?,
    }
    if let Some(b) = ui.banner() {
        chrome::banner(d, b, t)?;
    }
    let [k0, k1, k2] = ui.hints(v);
    chrome::top_bar(d, ui, k0, v, t)?;
    chrome::bottom_bar(d, ui, k1, k2, t)
}

// --- values ------------------------------------------------------------------------------------------------------

/// Unknown value.
const NONE: &str = "–";
/// Smallest slot of a speed: the comma stays in place from 9,8 to 32,4 km/h.
const SPEED: &str = "00,0";

pub(crate) fn km(cm: u32) -> Fmt {
    let tenths = cm / 10_000;
    Fmt::new(format_args!("{},{}", tenths / 10, tenths % 10))
}

pub(crate) fn kmh(mm_s: Option<u32>) -> Fmt {
    match mm_s {
        Some(v) => {
            let tenths = (v * 36 + 500) / 1000;
            Fmt::new(format_args!("{},{}", tenths / 10, tenths % 10))
        }
        None => Fmt::new(format_args!("{NONE}")),
    }
}

/// A distance along the route: in metres under 1 km (by 10 m), then in km with one decimal (docs/PRODUCT.md).
pub(crate) fn distance(dm: u32) -> Fmt {
    if dm < 10_000 {
        Fmt::new(format_args!("{} m", dm / 100 * 10))
    } else {
        Fmt::new(format_args!("{},{} km", dm / 10_000, dm / 1_000 % 10))
    }
}

fn duration(s: u32) -> Fmt {
    Fmt::new(format_args!(
        "{}:{:02}:{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60
    ))
}

fn int<T: core::fmt::Display>(v: Option<T>) -> Fmt {
    match v {
        Some(v) => Fmt::new(format_args!("{v}")),
        None => Fmt::new(format_args!("{NONE}")),
    }
}

fn clock(c: Option<(u8, u8)>) -> Fmt {
    match c {
        Some((h, m)) => Fmt::new(format_args!("{h:02}:{m:02}")),
        None => Fmt::new(format_args!("{NONE}")),
    }
}

/// `text` cut with "…" to fit in `width` pixels.
pub(crate) fn fit(font: &Font, text: &str, width: i32) -> Fmt {
    if font.width(text, 0) <= width {
        return Fmt::new(format_args!("{text}"));
    }
    let mut end = text.len();
    loop {
        end = text[..end].char_indices().last().map_or(0, |(i, _)| i);
        let cut = Fmt::new(format_args!("{}…", text[..end].trim_end()));
        if end == 0 || font.width(cut.as_str(), 0) <= width {
            return cut;
        }
    }
}

fn char_str(c: char, buf: &mut [u8; 4]) -> &str {
    c.encode_utf8(buf)
}

pub(crate) fn rect(x0: i32, y0: i32, x1: i32, y1: i32) -> Rectangle {
    Rectangle::with_corners(Point::new(x0, y0), Point::new(x1, y1))
}


