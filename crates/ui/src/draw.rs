//! Drawing of the screens: a top bar (top left key, clock, battery), the content, a bottom bar (the two bottom keys,
//! the ride page). Fonts, sizes, colours and positions from the prototype (docs/prototype), measured in a browser at
//! 240x320; a few alignments fixed on the way (ink-aligned values, icons on the capitals).
//!
//! Vertical positions are the centres of the capitals ([`fonts::text`]), horizontal ones the edges of the ink.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};

use crate::fonts::{self, Align, Font, Line as Fmt, Style, icon};
use crate::{Gnss, Hint, Kind, Screen, Ui, View};

const W: i32 = 240;
const H: i32 = 320;
/// Rule under the top bar, rule over the bottom bar.
const TOP: i32 = 24;
const BOTTOM: i32 = 279;
/// Side margin of the text.
const MARGIN: i32 = 12;

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
    pub notice_bg: Rgb565,
    pub notice_fg: Rgb565,
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
    warn: rgb(0xFFA94D),
    danger: rgb(0xFF8787),
    notice_bg: rgb(0xF2F2F2),
    notice_fg: rgb(0x111111),
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
        Screen::Status => {}
        Screen::Ride if ui.page() == 0 => ride_main(d, v, t)?,
        Screen::Ride => ride_altitude(d, v, t)?,
        Screen::Paused => paused(d, v, t)?,
        Screen::ConfirmStop => confirm_stop(d, v, t)?,
        Screen::Saved => saved(d, v, t)?,
        Screen::PowerOff => power_off(d, ui.power_off_saves_ride(), t)?,
    }
    if let Some(text) = ui.notice() {
        fill(
            d,
            Rectangle::new(Point::new(0, BOTTOM - 40), Size::new(W as u32, 40)),
            t.notice_bg,
        )?;
        let s = Style::new(&fonts::SUB, t.notice_fg, t.notice_bg);
        fonts::text(d, &s, text, W / 2, BOTTOM - 20, Align::Center)?;
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

// --- values ------------------------------------------------------------------------------------------------------

/// Unknown value.
const NONE: &str = "–";

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

/// Rule under the big cell of the ride pages, rule between the two rows of the grid.
const BIG_RULE: i32 = 132;
const GRID_RULE: i32 = 206;

fn ride_page<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    big: (&str, &str, &str),
    cells: [(&str, &str, &str); 4],
) -> Result<(), D::Error> {
    let area = Rectangle::with_corners(Point::new(0, TOP + 1), Point::new(W - 1, BIG_RULE - 1));
    cell(
        d,
        area,
        (&t.label(&fonts::LABEL), big.0),
        (&t.style(&fonts::HUGE), big.1, big.2),
        11,
    )?;
    hline(d, BIG_RULE, t)?;
    hline(d, GRID_RULE, t)?;
    vline(d, W / 2, BIG_RULE + 1, BOTTOM - 1, t)?;
    let rows = [(BIG_RULE + 1, GRID_RULE - 1), (GRID_RULE + 1, BOTTOM - 1)];
    for (i, (label, value, min)) in cells.into_iter().enumerate() {
        let (y0, y1) = rows[i / 2];
        let x0 = if i % 2 == 0 { 0 } else { W / 2 + 1 };
        let area = Rectangle::with_corners(Point::new(x0, y0), Point::new(x0 + W / 2 - 2, y1));
        cell(
            d,
            area,
            (&t.label(&fonts::LABEL_SMALL), label),
            (&t.style(&fonts::VALUE), value, min),
            9,
        )?;
    }
    Ok(())
}

/// Smallest slot of a speed: the comma stays in place from 9,8 to 32,4 km/h.
const SPEED: &str = "00,0";

/// Page 1: speed, then time, distance, D+ and heart rate.
fn ride_main<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    ride_page(
        d,
        t,
        ("VITESSE KM/H", kmh(v.speed_mm_s).as_str(), SPEED),
        [
            ("DURÉE", duration(v.ride.timer_s).as_str(), ""),
            ("DISTANCE KM", km(v.ride.distance_cm).as_str(), "00,0"),
            ("D+ M", int(Some(v.ride.ascent_m)).as_str(), "000"),
            ("FC BPM", int(v.heart_rate).as_str(), "000"),
        ],
    )
}

/// Page 2: altitude, then averages, clock and D-.
fn ride_altitude<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    ride_page(
        d,
        t,
        ("ALTITUDE M", int(v.altitude_dm.map(|a| a / 10)).as_str(), "000"),
        [
            ("MOYENNE KM/H", kmh(v.ride.avg_speed_mm_s()).as_str(), SPEED),
            ("MAX KM/H", kmh(Some(v.ride.max_speed_mm_s)).as_str(), SPEED),
            ("HEURE", clock(v.clock).as_str(), ""),
            ("D- M", int(Some(v.ride.descent_m)).as_str(), "000"),
        ],
    )
}

// --- other screens -----------------------------------------------------------------------------------------------

/// Label on the left, value on the right, on one line centred on `cy`.
fn row<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    (label, text): (&Style, &str),
    (value, v): (&Style, &str),
    cy: i32,
) -> Result<(), D::Error> {
    fonts::text(d, label, text, MARGIN, cy, Align::Left)?;
    // by the advance: a changing value keeps its right edge
    fonts::value(d, value, v, "", W - MARGIN, cy, Align::Right)?;
    Ok(())
}

fn home<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    fonts::text(d, &t.label(&fonts::LABEL), "TRACE", MARGIN, 45, Align::Left)?;
    fonts::text(
        d,
        &t.style(&fonts::HEADING),
        "Sortie libre",
        MARGIN,
        73,
        Align::Left,
    )?;
    fonts::text(
        d,
        &t.mute(&fonts::NOTE),
        "aucune trace chargée",
        MARGIN,
        99,
        Align::Left,
    )?;
    hline(d, 128, t)?;

    let label = t.mute(&fonts::ROW);
    let value = t.style(&fonts::ROW);
    let gnss = match v.gnss {
        Gnss::Off => (t.mute, Fmt::new(format_args!("éteint"))),
        Gnss::Searching { in_view } => (
            t.warn,
            Fmt::new(format_args!("{} recherche · {in_view} vus", icon::DOT)),
        ),
        Gnss::Fix { used } => (
            t.ok,
            Fmt::new(format_args!("{} fix · {used} sat", icon::DOT)),
        ),
    };
    let y = |i: i32| 158 + i * 34;
    row(
        d,
        (&label, "GPS"),
        (&value.colored(gnss.0), gnss.1.as_str()),
        y(0),
    )?;
    let alt = match v.altitude_dm {
        Some(a) => Fmt::new(format_args!("{} m", a / 10)),
        None => Fmt::new(format_args!("{NONE}")),
    };
    row(d, (&label, "Altitude"), (&value, alt.as_str()), y(1))?;
    row(
        d,
        (&label, "Sorties"),
        (&value, int(v.rides_stored).as_str()),
        y(2),
    )?;
    let free = match v.free_hours {
        Some(h) => Fmt::new(format_args!("~{h} h d'enregistrement")),
        None => Fmt::new(format_args!("{NONE}")),
    };
    row(d, (&label, "Place"), (&value, free.as_str()), y(3))
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
    fill(
        d,
        Rectangle::new(Point::new(0, TOP + 1), Size::new(W as u32, BAND as u32)),
        bg,
    )?;
    let (ts, ss) = (
        Style::new(&fonts::TITLE, fg, bg),
        Style::new(&fonts::SUB, fg, bg),
    );
    let mut buf = [0; 4];
    let mark = char_str(mark, &mut buf);
    // the icon as tall as the title and the line under it
    let ms = Style::new(&fonts::MARK, fg, bg);
    let mark_w = fonts::MARK.width(mark, 0);
    let text_w = fonts::TITLE.width(title, 0).max(fonts::SUB.width(sub, 0));
    let gap = 12;
    let left = (W - (mark_w + gap + text_w)) / 2;
    let (tc, sc, line_gap) = (fonts::TITLE.cap as i32, fonts::SUB.cap as i32, 9);
    let top = TOP + 1 + (BAND - (tc + line_gap + sc) + 1) / 2;
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

fn paused<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    band(d, t.kinds[1], icon::PAUSE, "En pause", "chrono arrêté")?;
    let label = t.mute(&fonts::ROW_SMALL);
    let value = t.style(&fonts::TITLE);
    let distance = Fmt::new(format_args!("{} km", km(v.ride.distance_cm).as_str()));
    let avg = Fmt::new(format_args!(
        "{} km/h",
        kmh(v.ride.avg_speed_mm_s()).as_str()
    ));
    row(d, (&label, "Distance"), (&value, distance.as_str()), 117)?;
    row(
        d,
        (&label, "Durée"),
        (&value, duration(v.ride.timer_s).as_str()),
        161,
    )?;
    row(d, (&label, "Moyenne"), (&value, avg.as_str()), 205)?;
    hline(d, 232, t)?;
    let note = "Éteindre le compteur termine\net enregistre la sortie.";
    fonts::lines(d, &t.mute(&fonts::NOTE), note, MARGIN, 249, 18, Align::Left)
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
    let r = fonts::text(d, s, mark, MARGIN, cy, Align::Left)?;
    // a space between the icon and the text, like the prototype
    let x = r.top_left.x + r.size.width as i32 + s.font.px as i32 / 4 + 1;
    fonts::lines(d, s, text, x, cy, line, Align::Left)
}

fn confirm_stop<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    // the prototype's red line: this screen ends something
    fill(
        d,
        Rectangle::new(Point::new(0, TOP + 1), Size::new(W as u32, 4)),
        t.danger,
    )?;
    fonts::text(
        d,
        &t.style(&fonts::HEADING),
        "Terminer la sortie ?",
        MARGIN,
        57,
        Align::Left,
    )?;
    let body = t.style(&fonts::BODY);
    let saved = Fmt::new(format_args!(
        "Enregistrée : {} km · {}",
        km(v.ride.distance_cm).as_str(),
        duration(v.ride.timer_s).as_str()
    ));
    bullet(d, &body, icon::CHECK, saved.as_str(), 95, 24)?;
    let ascent = Fmt::new(format_args!("D+ {} m", v.ride.ascent_m));
    bullet(d, &body, icon::CHECK, ascent.as_str(), 125, 24)?;
    let danger = t.style(&fonts::KEY).colored(t.danger);
    bullet(
        d,
        &danger,
        icon::CROSS,
        "Elle ne pourra plus\nêtre reprise.",
        163,
        24,
    )
}

fn saved<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    let mut buf = [0; 4];
    let check = char_str(icon::CHECK, &mut buf);
    fonts::text(
        d,
        &t.style(&fonts::MARK).colored(t.ok),
        check,
        W / 2,
        100,
        Align::Center,
    )?;
    fonts::text(
        d,
        &t.style(&fonts::HEADING),
        "Sortie enregistrée",
        W / 2,
        152,
        Align::Center,
    )?;
    let summary = Fmt::new(format_args!(
        "{} km · {} · {} m D+",
        km(v.ride.distance_cm).as_str(),
        duration(v.ride.timer_s).as_str(),
        v.ride.ascent_m
    ));
    fonts::text(
        d,
        &t.style(&fonts::BODY),
        summary.as_str(),
        W / 2,
        184,
        Align::Center,
    )?;
    let avg = Fmt::new(format_args!(
        "moyenne {} km/h",
        kmh(v.ride.avg_speed_mm_s()).as_str()
    ));
    fonts::text(
        d,
        &t.mute(&fonts::NOTE),
        avg.as_str(),
        W / 2,
        210,
        Align::Center,
    )?;
    Ok(())
}

fn power_off<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    saves_ride: bool,
    t: &Theme,
) -> Result<(), D::Error> {
    fonts::text(
        d,
        &t.style(&fonts::HEADING),
        "Éteindre le compteur ?",
        MARGIN,
        57,
        Align::Left,
    )?;
    if saves_ride {
        let text = "La sortie en pause sera\nenregistrée.";
        bullet(d, &t.style(&fonts::BODY), icon::CHECK, text, 95, 24)?;
    }
    let note = "Sans réponse, le compteur reste\nallumé dans 8 s.";
    fonts::lines(d, &t.mute(&fonts::NOTE), note, MARGIN, 249, 18, Align::Left)
}

// --- bars --------------------------------------------------------------------------------------------------------

fn kind_colors(t: &Theme, kind: Kind) -> (Rgb565, Rgb565) {
    match kind {
        Kind::Plain => (t.chip, t.fg),
        Kind::Go => t.kinds[0],
        Kind::Pause => t.kinds[1],
        Kind::Danger => t.kinds[2],
        Kind::Select => t.kinds[3],
    }
}

/// Padding of the key labels in their box: the margin of the content, so that labels, rows and values share their
/// left and right edges.
const PAD: i32 = MARGIN;

fn top_bar<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    hint: Option<Hint>,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    let cy = TOP / 2;
    if let Some(h) = hint {
        let (bg, fg) = kind_colors(t, h.kind);
        let w = fonts::HINT.width(h.text, 0) + 2 * PAD;
        fill(
            d,
            Rectangle::new(Point::zero(), Size::new(w as u32, TOP as u32)),
            bg,
        )?;
        fonts::text(
            d,
            &Style::new(&fonts::HINT, fg, bg),
            h.text,
            PAD,
            cy,
            Align::Left,
        )?;
    }
    let bar = t.mute(&fonts::BAR);
    if let Some(p) = v.battery_pct {
        let pct = Fmt::new(format_args!("{p} %"));
        fonts::value(d, &bar, pct.as_str(), "", W - MARGIN, cy, Align::Right)?;
        // the icon stays where "100 %" would start
        battery(d, W - MARGIN - fonts::BAR.width("100 %", 0) - 5, cy, p, t)?;
    }
    // the clock in the middle of the bar
    fonts::value(d, &bar, clock(v.clock).as_str(), "00:00", W / 2, cy, Align::Center)?;
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
    if level > 0 {
        let c = if pct <= 15 { t.danger } else { t.mute };
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

fn bottom_bar<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    left: Option<Hint>,
    right: Option<Hint>,
    page: Option<u8>,
    t: &Theme,
) -> Result<(), D::Error> {
    let h = H - BOTTOM - 1;
    let cy = BOTTOM + 1 + h / 2;
    let middle = if page.is_some() { 38 } else { 12 };
    let half = (W - middle) / 2;
    for (hint, x0, align, x) in [
        (left, 0, Align::Left, PAD),
        (right, W - half, Align::Right, W - PAD),
    ] {
        let (bg, fg) = hint.map_or((t.chip, t.fg), |h| kind_colors(t, h.kind));
        fill(
            d,
            Rectangle::new(Point::new(x0, BOTTOM + 1), Size::new(half as u32, h as u32)),
            bg,
        )?;
        if let Some(hint) = hint {
            debug_assert!(
                fonts::KEY.width(hint.text, 0) + 2 * PAD <= half,
                "{:?} too wide",
                hint.text
            );
            fonts::text(d, &Style::new(&fonts::KEY, fg, bg), hint.text, x, cy, align)?;
        }
    }
    if let Some(p) = page {
        let mut buf = [0; 4];
        let dot = char_str(icon::DOT, &mut buf);
        for i in 0..crate::PAGES {
            let x = W / 2 + (2 * i as i32 - (crate::PAGES as i32 - 1)) * 5;
            let c = if i == p { t.fg } else { t.rule };
            fonts::text(
                d,
                &Style::new(&fonts::LABEL, c, t.bg),
                dot,
                x,
                cy,
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
    Line::new(Point::new(0, y), Point::new(W - 1, y))
        .into_styled(PrimitiveStyle::with_stroke(t.rule, 1))
        .draw(d)
}

fn vline<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    x: i32,
    y0: i32,
    y1: i32,
    t: &Theme,
) -> Result<(), D::Error> {
    Line::new(Point::new(x, y0), Point::new(x, y1))
        .into_styled(PrimitiveStyle::with_stroke(t.rule, 1))
        .draw(d)
}
