//! Drawing of the screens: a top bar (top left key, clock, battery), the content, a bottom bar in three fixed
//! columns (bottom left key, ride page, bottom right key). Fonts, sizes, colours and positions from the prototype
//! (docs/prototype), measured with tools/prototype_shots.py; a few alignments fixed on the way (values by their
//! advance, label and value of a row on one baseline, icons on the capitals).
//!
//! Vertical positions are the centres of the capitals ([`fonts::text`]), horizontal ones the edges of the ink.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};

use crate::fonts::{self, Align, Font, Line as Fmt, Style, icon};
use crate::{BannerKind, Gnss, Hint, Kind, Page, Screen, Ui, View};

const W: i32 = 240;
const H: i32 = 320;
/// Rule under the top bar, rule over the key bar.
const TOP: i32 = 30;
const BOTTOM: i32 = 279;
/// Side margin of the content.
const MARGIN: i32 = 12;
/// Side margin of the key labels, in their boxes.
const KEY_PAD: i32 = 8;
/// The blue mark of a key box, on the side of its key.
const KEY_MARK: i32 = 5;
/// Width of the top key box: the middle of the top bar stays free for the clock.
const TOP_KEY: i32 = 100;
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
}

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
        Screen::Menu => menu(d, ui, v, t, None, MENU_NOTE)?,
        Screen::PauseMenu => menu(d, ui, v, t, Some("SORTIE EN PAUSE"), PAUSE_MENU_NOTE)?,
        Screen::Status => {}
        Screen::Ride => match ui.page() {
            Some(Page::Altitude) => ride_altitude(d, v, t)?,
            Some(Page::Essential) | None => ride_essential(d, v, t)?,
        },
        Screen::Paused => paused(d, v, t, "En pause", "chrono arrêté", PAUSED_NOTE)?,
        Screen::Resume => paused(
            d,
            v,
            t,
            "Sortie en pause",
            "le compteur était éteint",
            RESUME_NOTE,
        )?,
        Screen::ConfirmStop => confirm_stop(d, v, t)?,
        Screen::Saved => saved(d, v, t)?,
    }
    if let Some(b) = ui.banner() {
        banner(d, b, t)?;
    }
    let [k0, k1, k2] = ui.hints(v);
    top_bar(d, ui, k0, v, t)?;
    bottom_bar(d, ui, k1, k2, t)
}

// --- values ------------------------------------------------------------------------------------------------------

/// Unknown value.
const NONE: &str = "–";
/// Smallest slot of a speed: the comma stays in place from 9,8 to 32,4 km/h.
const SPEED: &str = "00,0";

fn km(cm: u32) -> Fmt {
    let tenths = cm / 10_000;
    Fmt::new(format_args!("{},{}", tenths / 10, tenths % 10))
}

fn kmh(mm_s: Option<u32>) -> Fmt {
    match mm_s {
        Some(v) => {
            let tenths = (v * 36 + 500) / 1000;
            Fmt::new(format_args!("{},{}", tenths / 10, tenths % 10))
        }
        None => Fmt::new(format_args!("{NONE}")),
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

fn char_str(c: char, buf: &mut [u8; 4]) -> &str {
    c.encode_utf8(buf)
}

fn rect(x0: i32, y0: i32, x1: i32, y1: i32) -> Rectangle {
    Rectangle::with_corners(Point::new(x0, y0), Point::new(x1, y1))
}

// --- ride pages --------------------------------------------------------------------------------------------------

/// A label over a value, the pair centred in the box (`gap` pixels between their capitals); the value keeps its
/// place when it changes, in a slot at least as wide as `min` ([`fonts::value`]).
fn cell<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    area: Rectangle,
    (label, text): (&Style, &str),
    (value, v, min): (&Style, &str, &str),
    gap: i32,
) -> Result<(), D::Error> {
    let (lc, vc) = (label.font.cap as i32, value.font.cap as i32);
    let top = area.top_left.y + (area.size.height as i32 - (lc + gap + vc) + 1) / 2;
    let x = area.top_left.x + area.size.width as i32 / 2;
    fonts::text(d, label, text, x, top + lc / 2, Align::Center)?;
    fonts::value(d, value, v, min, x, top + lc + gap + vc / 2, Align::Center)?;
    Ok(())
}

/// Page Essential: speed, then time, distance, D+ and heart rate (else the average speed).
fn ride_essential<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    const BIG: i32 = 139;
    const MID: i32 = 209;
    cell(
        d,
        rect(0, TOP + 1, W - 1, BIG - 1),
        (&t.label(&fonts::LABEL), "VITESSE KM/H"),
        (&t.style(&fonts::HUGE), kmh(v.speed_mm_s).as_str(), SPEED),
        16,
    )?;
    hline(d, BIG, t)?;
    hline(d, MID, t)?;
    vline(d, W / 2 - 1, BIG + 1, BOTTOM - 1, t)?;
    let small = t.label(&fonts::LABEL_SMALL);
    let value = t.style(&fonts::VALUE);
    let last = match v.heart_rate {
        Some(hr) => (small, "FC BPM", int(Some(hr)), "000"),
        // no heart rate: the average speed takes its place, its label in the text colour like the prototype
        None => (
            small.colored(t.fg),
            "MOYENNE KM/H",
            kmh(v.ride.avg_speed_mm_s()),
            SPEED,
        ),
    };
    let cells = [
        (small, "DURÉE", duration(v.ride.timer_s), ""),
        (small, "DISTANCE KM", km(v.ride.distance_cm), "00,0"),
        (small, "D+ M", int(Some(v.ride.ascent_m)), "000"),
        last,
    ];
    for (i, (label, text, val, min)) in cells.iter().enumerate() {
        let (y0, y1) = if i < 2 {
            (BIG + 1, MID - 1)
        } else {
            (MID + 1, BOTTOM - 1)
        };
        let (x0, x1) = if i % 2 == 0 {
            (0, W / 2 - 2)
        } else {
            (W / 2, W - 1)
        };
        cell(
            d,
            rect(x0, y0, x1, y1),
            (label, text),
            (&value, val.as_str(), min),
            8,
        )?;
    }
    Ok(())
}

/// Page Altitude: altitude, then gradient, D+ and D-, then the last 5 km.
fn ride_altitude<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    const BIG: i32 = 131;
    const ROW: i32 = 194;
    cell(
        d,
        rect(0, TOP + 1, W - 1, BIG - 1),
        (&t.label(&fonts::LABEL), "ALTITUDE M"),
        (
            &t.style(&fonts::ALT),
            int(v.altitude_dm.map(|a| a / 10)).as_str(),
            "000",
        ),
        12,
    )?;
    hline(d, BIG, t)?;
    hline(d, ROW, t)?;
    vline(d, 79, BIG + 1, ROW - 1, t)?;
    vline(d, 159, BIG + 1, ROW - 1, t)?;
    let small = t.label(&fonts::LABEL_SMALL);
    let value = t.style(&fonts::VALUE);
    let grade = match v.grade_pct {
        Some(g) => Fmt::new(format_args!("{g} %")),
        None => Fmt::new(format_args!("{NONE}")),
    };
    let grade_style = value.colored(v.grade_pct.map_or(t.fg, |g| t.grade(g)));
    let cells = [
        ("PENTE", grade, grade_style, "0 %"),
        ("D+ M", int(Some(v.ride.ascent_m)), value, "000"),
        ("D- M", int(Some(v.ride.descent_m)), value, "000"),
    ];
    for (i, (label, val, style, min)) in cells.iter().enumerate() {
        let x0 = [0, 80, 160][i];
        cell(
            d,
            rect(x0, BIG + 1, x0 + 78, ROW - 1),
            (&small, label),
            (style, val.as_str(), min),
            8,
        )?;
    }
    fonts::text(
        d,
        &t.label(&fonts::LABEL_TINY),
        "5 DERNIERS KM",
        8,
        205,
        Align::Left,
    )?;
    profile(d, rect(8, 214, 232, BOTTOM - 2), v.recent.points(), true, t)
}

/// An altitude profile: the area under it; with `me` (riding), the line and the rider at its end.
fn profile<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    area: Rectangle,
    points: &[i16],
    me: bool,
    t: &Theme,
) -> Result<(), D::Error> {
    if points.len() < 2 {
        return Ok(());
    }
    let (lo, hi) = points
        .iter()
        .fold((i16::MAX, i16::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    // at least 20 m from bottom to top: a flat road stays flat
    let span = ((hi as i32 - lo as i32).max(200)) as i64;
    let mid = (lo as i32 + hi as i32) / 2;
    let (x0, y0) = (area.top_left.x, area.top_left.y);
    let (w, h) = (area.size.width as i32 - 1, area.size.height as i32 - 1);
    // room for the rider's point at the right end and the top
    let (w_line, top) = if me { (w - 4, 5) } else { (w, 1) };
    let inner = (h - top - 2) as i64;
    let y_of = |alt: i32| y0 + top + (h - top) / 2 - ((alt - mid) as i64 * inner / span) as i32;
    let n = points.len() as i64 - 1;
    let at = |x: i32| {
        // the altitude under column x, linear between the points (sixteenths of a step)
        let pos = x as i64 * n * 16 / w_line.max(1) as i64;
        let i = (pos / 16).min(n - 1);
        let f = pos - i * 16;
        let (a, b) = (points[i as usize] as i64, points[i as usize + 1] as i64);
        y_of((a + (b - a) * f / 16) as i32)
    };
    for x in 0..=w_line {
        fill(d, rect(x0 + x, at(x), x0 + x, y0 + h), t.profile)?;
    }
    for x in 0..w_line * me as i32 {
        Line::new(Point::new(x0 + x, at(x)), Point::new(x0 + x + 1, at(x + 1)))
            .into_styled(PrimitiveStyle::with_stroke(t.fg, 2))
            .draw(d)?;
    }
    if me {
        disc(d, (x0 + w_line) as f32, at(w_line) as f32, 4.0, t.me, t.bg)?;
    }
    Ok(())
}

// --- other screens -----------------------------------------------------------------------------------------------

/// Label on the left, value on the right, on the baseline of the value (centred on `cy`).
fn row<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    (label, text): (&Style, &str),
    (value, v): (&Style, &str),
    cy: i32,
    margin: i32,
) -> Result<(), D::Error> {
    let baseline = cy + (value.font.cap as i32 + 1) / 2;
    let label_cy = baseline - (label.font.cap as i32 + 1) / 2;
    fonts::text(d, label, text, margin, label_cy, Align::Left)?;
    // by the advance: a changing value keeps its right edge
    fonts::value(d, value, v, "", W - margin, cy, Align::Right)?;
    Ok(())
}

fn home<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    fonts::text(d, &t.label(&fonts::LABEL), "TRACE", MARGIN, 47, Align::Left)?;
    // the route's colour mark, grey without a route
    rounded(d, rect(MARGIN, 65, 30, 68), 2, t.rule, t.bg)?;
    fonts::text(
        d,
        &t.style(&fonts::HEADING_SMALL),
        "Aucune trace",
        38,
        66,
        Align::Left,
    )?;
    let note = "Envoie-en une depuis l’app (facultatif)";
    fonts::text(
        d,
        &t.mute(&fonts::NOTE_LARGE),
        note,
        MARGIN,
        111,
        Align::Left,
    )?;

    let label = t.mute(&fonts::ROW);
    let value = t.style(&fonts::ROW);
    let gnss = match v.gnss {
        Gnss::Off => (t.mute, Fmt::new(format_args!("éteint"))),
        Gnss::Searching { in_view, .. } => (
            t.warn,
            Fmt::new(format_args!("{} Recherche · {in_view} sat", icon::RING)),
        ),
        Gnss::Fix { used } => (
            t.ok,
            Fmt::new(format_args!("{} Fix OK · {used} sat", icon::DOT)),
        ),
    };
    row(
        d,
        (&label, "GPS"),
        (&value.colored(gnss.0), gnss.1.as_str()),
        143,
        MARGIN,
    )?;
    if let Gnss::Searching { progress, .. } = v.gnss {
        rounded(d, rect(MARGIN, 156, W - MARGIN - 1, 159), 2, t.chip, t.bg)?;
        let w = (W - 2 * MARGIN) * progress.min(100) as i32 / 100;
        if w > 3 {
            rounded(
                d,
                rect(MARGIN, 156, MARGIN + w - 1, 159),
                2,
                t.attention,
                t.chip,
            )?;
        }
    }
    row(
        d,
        (&label, "Capteurs"),
        (&value.colored(t.ok), "aucun (GPS seul)"),
        176,
        MARGIN,
    )?;
    row(
        d,
        (&label, "Pages"),
        (&value, crate::pages().names().as_str()),
        203,
        MARGIN,
    )?;
    let pending = match v.rides_stored {
        Some(0) => (t.fg, Fmt::new(format_args!("aucune"))),
        Some(1) => (t.warn, Fmt::new(format_args!("1 sortie"))),
        Some(n) => (t.warn, Fmt::new(format_args!("{n} sorties"))),
        None => (t.mute, Fmt::new(format_args!("{NONE}"))),
    };
    row(
        d,
        (&label, "À envoyer"),
        (&value.colored(pending.0), pending.1.as_str()),
        230,
        MARGIN,
    )
}

const MENU_NOTE: (&str, &str) = (
    "Dans l'app :",
    "envoi de la trace, pages de données,\nzones FC et FTP, roue et manivelle,\nréseaux Wi-Fi et serveur de synchro.",
);
const PAUSE_MENU_NOTE: (&str, &str) = (
    "",
    "Pour changer de trace, envoie-la depuis\nl'app, même pendant la sortie.",
);

/// A menu: an optional title, the entries (the selected one in blue), a note.
fn menu<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    v: &View,
    t: &Theme,
    title: Option<&str>,
    (note_title, note): (&str, &str),
) -> Result<(), D::Error> {
    const ROW: i32 = 43;
    let mut y = TOP + 1;
    if let Some(title) = title {
        fonts::text(d, &t.label(&fonts::LABEL), title, MARGIN, 47, Align::Left)?;
        y = 62;
    }
    let (items, sel) = ui.menu(v.resumable);
    let mut buf = [0; 4];
    let chevron = char_str(icon::CHEVRON, &mut buf);
    for (i, item) in items.iter().flatten().enumerate() {
        let (bg, fg) = if i as u8 == sel {
            (t.sel, t.kinds[3].1)
        } else {
            (t.bg, t.fg)
        };
        fill(d, rect(0, y, W - 1, y + ROW - 2), bg)?;
        hline(d, y + ROW - 1, t)?;
        let s = Style::new(&fonts::MENU, fg, bg);
        let cy = y + ROW / 2 - 1;
        fonts::text(d, &s, item.label(), MARGIN, cy, Align::Left)?;
        fonts::text(d, &s, chevron, W - MARGIN, cy, Align::Right)?;
        y += ROW;
    }
    let mut cy = y + 20;
    if !note_title.is_empty() {
        fonts::text(
            d,
            &t.mute(&fonts::NOTE_BOLD),
            note_title,
            MARGIN,
            cy,
            Align::Left,
        )?;
        cy += 21;
    }
    note_lines(d, &t.mute(&fonts::NOTE), note, MARGIN, cy, 19)
}

/// Height of the coloured band under the top bar.
const BAND: i32 = 64;

/// Coloured band under the top bar: an icon, a title and a line under it, the group centred.
fn band<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    (bg, fg): (Rgb565, Rgb565),
    mark: char,
    title: &str,
    sub: &str,
) -> Result<(), D::Error> {
    fill(d, rect(0, TOP + 1, W - 1, TOP + BAND), bg)?;
    let (ts, ss, ms) = (
        Style::new(&fonts::TITLE, fg, bg),
        Style::new(&fonts::SUB, fg, bg),
        Style::new(&fonts::MARK, fg, bg),
    );
    let mut buf = [0; 4];
    let mark = char_str(mark, &mut buf);
    let mark_w = fonts::MARK.width(mark, 0);
    let text_w = fonts::TITLE.width(title, 0).max(fonts::SUB.width(sub, 0));
    let gap = 12;
    let left = (W - (mark_w + gap + text_w)) / 2;
    let (tc, sc, line_gap) = (fonts::TITLE.cap as i32, fonts::SUB.cap as i32, 9);
    let top = TOP + 1 + (BAND - (tc + line_gap + sc) + 1) / 2;
    // the icon as tall as the title and the line under it
    fonts::text(
        d,
        &ms,
        mark,
        left,
        top + (tc + line_gap + sc) / 2,
        Align::Left,
    )?;
    fonts::text(
        d,
        &ts,
        title,
        left + mark_w + gap,
        top + tc / 2,
        Align::Left,
    )?;
    fonts::text(
        d,
        &ss,
        sub,
        left + mark_w + gap,
        top + tc + line_gap + sc / 2,
        Align::Left,
    )?;
    Ok(())
}

const PAUSED_NOTE: &str = "Pause longue ? Menu › Éteindre :\nla sortie reprend au rallumage.";
const RESUME_NOTE: &str = "Le temps éteint ne compte pas\ndans la durée.";

/// The pause, and the paused ride found at power-on.
fn paused<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
    title: &str,
    sub: &str,
    note: &str,
) -> Result<(), D::Error> {
    band(d, t.kinds[1], icon::PAUSE, title, sub)?;
    let label = t.mute(&fonts::ROW_SMALL);
    let value = t.style(&fonts::TITLE);
    let distance = Fmt::new(format_args!("{} km", km(v.ride.distance_cm).as_str()));
    let avg = Fmt::new(format_args!(
        "{} km/h",
        kmh(v.ride.avg_speed_mm_s()).as_str()
    ));
    let m = 14;
    row(d, (&label, "Distance"), (&value, distance.as_str()), 123, m)?;
    row(
        d,
        (&label, "Durée"),
        (&value, duration(v.ride.timer_s).as_str()),
        167,
        m,
    )?;
    row(d, (&label, "Moyenne"), (&value, avg.as_str()), 211, m)?;
    note_lines(d, &t.mute(&fonts::NOTE), note, m, 246, 18)
}

/// Lines of a note in the content, checked to stay above the key bar.
fn note_lines<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Style,
    text: &str,
    x: i32,
    cy: i32,
    line: i32,
) -> Result<(), D::Error> {
    let last = cy + (text.lines().count() as i32 - 1) * line;
    debug_assert!(
        last + s.font.descent as i32 + s.font.cap as i32 / 2 < BOTTOM,
        "{text:?} runs into the key bar"
    );
    fonts::lines(d, s, text, x, cy, line, Align::Left)
}

/// An icon and text lines after it, the lines after the first one under the text (hanging indent).
fn bullet<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    s: &Style,
    mark: char,
    text: &str,
    cy: i32,
    line: i32,
) -> Result<(), D::Error> {
    let mut buf = [0; 4];
    let mark = char_str(mark, &mut buf);
    let r = fonts::text(d, s, mark, 14, cy, Align::Left)?;
    // a space between the icon and the text, like the prototype
    let x = r.top_left.x + r.size.width as i32 + s.font.px as i32 / 4 + 1;
    fonts::lines(d, s, text, x, cy, line, Align::Left)
}

fn confirm_stop<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    // the prototype's blue line: confirmed with the blue key
    fill(d, rect(0, TOP + 1, W - 1, TOP + 6), t.sel)?;
    fonts::text(
        d,
        &t.style(&fonts::HEADING),
        "Terminer la sortie ?",
        14,
        63,
        Align::Left,
    )?;
    let saved = Fmt::new(format_args!(
        "Enregistrée : {} km · {}",
        km(v.ride.distance_cm).as_str(),
        duration(v.ride.timer_s).as_str()
    ));
    bullet(
        d,
        &t.style(&fonts::BODY),
        icon::CHECK,
        saved.as_str(),
        96,
        22,
    )?;
    let resumable = "Reprenable tant qu'elle n'est\npas envoyée";
    bullet(
        d,
        &t.style(&fonts::BODY_BOLD),
        icon::UNDO,
        resumable,
        128,
        22,
    )
}

fn saved<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    let title = Fmt::new(format_args!("{} Sortie enregistrée", icon::CHECK));
    fonts::text(
        d,
        &t.style(&fonts::HEADING).colored(t.ok),
        title.as_str(),
        W / 2,
        58,
        Align::Center,
    )?;
    let label = t.label(&fonts::LABEL_SMALL);
    let value = t.style(&fonts::STAT);
    let distance = Fmt::new(format_args!("{} km", km(v.ride.distance_cm).as_str()));
    let ascent = Fmt::new(format_args!("{} m", v.ride.ascent_m));
    let stats = [
        ("DISTANCE", distance),
        ("DURÉE", duration(v.ride.timer_s)),
        ("MOYENNE", kmh(v.ride.avg_speed_mm_s())),
        ("D+", ascent),
    ];
    for (i, (l, val)) in stats.iter().enumerate() {
        let x = if i % 2 == 0 { 14 } else { 125 };
        let y = if i < 2 { 86 } else { 128 };
        fonts::text(d, &label, l, x, y, Align::Left)?;
        fonts::value(d, &value, val.as_str(), "", x, y + 19, Align::Left)?;
    }
    profile(d, rect(14, 166, 226, 200), v.whole.points(), false, t)?;
    let stopped = v.ride.elapsed_s.saturating_sub(v.ride.timer_s) / 60;
    if stopped > 0 {
        let line = Fmt::new(format_args!("{stopped} min à l'arrêt"));
        fonts::text(
            d,
            &t.style(&fonts::SUB),
            line.as_str(),
            W / 2,
            215,
            Align::Center,
        )?;
    }
    if v.resumable {
        let note = "Tu peux la reprendre (bas gauche,\nou Menu) tant qu'elle n'est pas envoyée.";
        let s = t.mute(&fonts::NOTE);
        for (i, l) in note.lines().enumerate() {
            fonts::text(d, &s, l, W / 2, 239 + i as i32 * 18, Align::Center)?;
        }
    }
    Ok(())
}

// --- banner ------------------------------------------------------------------------------------------------------

fn banner<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    b: &crate::Banner,
    t: &Theme,
) -> Result<(), D::Error> {
    let area = rect(6, 221, 233, 272);
    rounded(d, area, 6, t.banner_bg, t.bg)?;
    let (circle, mark, mark_fg) = match b.kind {
        BannerKind::Info => (t.info, "i", rgb(0xFFFFFF)),
        BannerKind::Attention => (t.attention, "!", rgb(0x111111)),
    };
    disc(d, 29.5, 246.5, 14.0, circle, t.banner_bg)?;
    fonts::text(
        d,
        &Style::new(&fonts::BANNER, mark_fg, circle),
        mark,
        30,
        247,
        Align::Center,
    )?;
    let s = Style::new(&fonts::BANNER, t.banner_fg, t.banner_bg);
    fonts::text(d, &s, b.title, 54, 235, Align::Left)?;
    let s = Style::new(&fonts::SUB, t.banner_fg, t.banner_bg);
    fonts::text(d, &s, b.detail.as_str(), 54, 255, Align::Left)?;
    if b.kind == BannerKind::Info {
        // what is left of its time, in tenths
        let w = (area.size.width as i32 - 12) * b.left_tenths as i32 / 10;
        if w > 0 {
            fill(d, rect(12, 268, 12 + w - 1, 269), t.info)?;
        }
    }
    Ok(())
}

// --- bars --------------------------------------------------------------------------------------------------------

fn top_bar<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    hint: Hint,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    let (bg, fg) = t.kind(hint.kind);
    fill(d, rect(0, 0, TOP_KEY - 1, TOP - 1), bg)?;
    fill(d, rect(0, 0, TOP_KEY - 1, KEY_MARK - 1), t.key_mark)?;
    key_label(
        d,
        hint.text,
        Style::new(&fonts::KEY, fg, bg),
        KEY_PAD,
        (KEY_MARK + TOP) / 2,
        Align::Left,
        TOP_KEY,
    )?;
    let cy = TOP / 2;
    let bar = t.mute(&fonts::BAR);
    if ui.gps_warning() {
        let gps = Fmt::new(format_args!("GPS {}", icon::CROSS));
        fonts::text(
            d,
            &bar.colored(t.warn),
            gps.as_str(),
            W / 2,
            cy,
            Align::Center,
        )?;
    } else {
        fonts::value(
            d,
            &bar,
            clock(v.clock).as_str(),
            "00:00",
            W / 2,
            cy,
            Align::Center,
        )?;
    }
    if let Some(p) = v.battery_pct {
        let pct = Fmt::new(format_args!("{p} %"));
        fonts::value(d, &bar, pct.as_str(), "", W - KEY_PAD, cy, Align::Right)?;
        // the icon stays where "100 %" would start
        battery(d, W - KEY_PAD - fonts::BAR.width("100 %", 0) - 5, cy, p, t)?;
    }
    hline(d, TOP, t)
}

const BATTERY_W: i32 = 18;

/// Battery outline with its level, right edge at `right`, centred on `cy`.
fn battery<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    right: i32,
    cy: i32,
    pct: u8,
    t: &Theme,
) -> Result<(), D::Error> {
    let (w, h) = (BATTERY_W - 3, 10);
    let left = right - BATTERY_W;
    let top = cy - h / 2;
    Rectangle::new(Point::new(left, top), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_stroke(t.mute, 1))
        .draw(d)?;
    fill(
        d,
        Rectangle::new(Point::new(left + w + 1, top + 3), Size::new(2, 4)),
        t.mute,
    )?;
    // at least a pixel while there is charge left
    let level = ((w - 4) * pct.min(100) as i32 / 100).max((pct > 0) as i32);
    let c = if pct <= 15 { t.danger } else { t.mute };
    if level > 0 {
        fill(
            d,
            Rectangle::new(
                Point::new(left + 2, top + 2),
                Size::new(level as u32, (h - 4) as u32),
            ),
            c,
        )?;
    }
    Ok(())
}

/// A key label, checked to fit in its box (`width`).
fn key_label<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    text: &str,
    s: Style,
    x: i32,
    cy: i32,
    align: Align,
    width: i32,
) -> Result<(), D::Error> {
    debug_assert!(
        fonts::KEY.width(text, 0) + 2 * KEY_PAD <= width,
        "key label {text:?} too wide"
    );
    fonts::text(d, &s, text, x, cy, align)?;
    Ok(())
}

fn bottom_bar<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    left: Hint,
    right: Hint,
    t: &Theme,
) -> Result<(), D::Error> {
    // the labels sit in the middle of the box above the mark
    let cy = (BOTTOM + 1 + H - KEY_MARK) / 2;
    for (hint, x0, x1, x, align) in [
        (left, 0, BAR_LEFT - 1, KEY_PAD, Align::Left),
        (right, BAR_RIGHT, W - 1, W - KEY_PAD, Align::Right),
    ] {
        let (bg, fg) = t.kind(hint.kind);
        fill(d, rect(x0, BOTTOM + 1, x1, H - 1), bg)?;
        fill(d, rect(x0, H - KEY_MARK, x1, H - 1), t.key_mark)?;
        key_label(
            d,
            hint.text,
            Style::new(&fonts::KEY, fg, bg),
            x,
            cy,
            align,
            x1 - x0 + 1,
        )?;
    }
    let (page, pages) = ui.page_index();
    if ui.screen() == Screen::Ride && pages > 1 {
        let mut buf = [0; 4];
        let mid = (BAR_LEFT + BAR_RIGHT) / 2;
        for i in 0..pages {
            let (c, dot) = if i == page {
                (t.fg, icon::DOT)
            } else {
                (t.mute, icon::RING)
            };
            let x = mid + (2 * i as i32 - (pages as i32 - 1)) * 5;
            let dot = char_str(dot, &mut buf);
            fonts::text(
                d,
                &Style::new(&fonts::LABEL_SMALL, c, t.bg),
                dot,
                x,
                cy,
                Align::Center,
            )?;
        }
        if ui.scrolling() {
            let cycle = char_str(icon::CYCLE, &mut buf);
            fonts::text(
                d,
                &t.mute(&fonts::LABEL_SMALL),
                cycle,
                mid,
                cy + 12,
                Align::Center,
            )?;
        }
    }
    hline(d, BOTTOM, t)
}

// --- primitives --------------------------------------------------------------------------------------------------

fn fill<D: DrawTarget<Color = Rgb565>>(d: &mut D, r: Rectangle, c: Rgb565) -> Result<(), D::Error> {
    d.fill_solid(&r, c)
}

fn hline<D: DrawTarget<Color = Rgb565>>(d: &mut D, y: i32, t: &Theme) -> Result<(), D::Error> {
    fill(d, rect(0, y, W - 1, y), t.rule)
}

fn vline<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    x: i32,
    y0: i32,
    y1: i32,
    t: &Theme,
) -> Result<(), D::Error> {
    fill(d, rect(x, y0, x, y1), t.rule)
}

/// Fills the pixels of `area` inside a shape, its edges anti-aliased (4x4 samples a pixel) into `bg`.
fn shape<D: DrawTarget<Color = Rgb565>>(
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
fn disc<D: DrawTarget<Color = Rgb565>>(
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
fn rounded<D: DrawTarget<Color = Rgb565>>(
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
