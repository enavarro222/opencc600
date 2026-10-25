use super::*;
use super::primitives::{disc, fill, hline, rounded};

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

/// Fewer hours of riding left than this: shown in red on the home screen.
const LOW_RECORD_HOURS: u32 = 5;

pub(super) fn home<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    fonts::text(d, &t.label(&fonts::LABEL), "TRACE", MARGIN, 47, Align::Left)?;
    // the route's colour mark, grey without a route followed
    let mark = if v.route.is_some() { t.route } else { t.rule };
    rounded(d, rect(MARGIN, 65, 30, 68), 2, mark, t.bg)?;
    let heading = t.style(&fonts::HEADING_SMALL);
    match &v.route {
        Some(r) => {
            let name = fit(&fonts::HEADING_SMALL, r.name.as_str(), W - 38 - MARGIN);
            fonts::text(d, &heading, name.as_str(), 38, 66, Align::Left)?;
            let info = Fmt::new(format_args!("{} km · {} m D+", km(r.length_dm * 10).as_str(), r.ascent_m));
            fonts::text(d, &t.mute(&fonts::NOTE_LARGE), info.as_str(), MARGIN, 111, Align::Left)?;
        }
        None if v.route_set_aside => {
            fonts::text(d, &heading, "Parcours libre", 38, 66, Align::Left)?;
            let note = Fmt::new(format_args!("Trace gardée · {} Menu pour la suivre", icon::MENU));
            fonts::text(d, &t.mute(&fonts::NOTE_LARGE), note.as_str(), MARGIN, 111, Align::Left)?;
        }
        None => {
            fonts::text(d, &heading, "Aucune trace", 38, 66, Align::Left)?;
            let note = "Envoie-en une depuis l’app (facultatif)";
            fonts::text(d, &t.mute(&fonts::NOTE_LARGE), note, MARGIN, 111, Align::Left)?;
        }
    }

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
    let roles = v.sensors.roles.iter().zip(Role::ALL);
    let connected = roles.clone().filter(|(r, _)| r.link == Link::Connected).count();
    let searching = roles.clone().filter(|(r, _)| r.link == Link::Searching).count();
    let sensors = match (connected, searching) {
        (0, 0) => (t.ok, Fmt::new(format_args!("aucun (GPS seul)"))),
        (c, 0) => {
            let mut names = Fmt::new(format_args!(""));
            for (i, (_, role)) in roles.filter(|(r, _)| r.link == Link::Connected).enumerate() {
                let _ = core::fmt::Write::write_fmt(
                    &mut names,
                    format_args!("{}{}", if i > 0 { ", " } else { "" }, role.label()),
                );
            }
            // the names when they fit, else how many
            if fonts::ROW.width(names.as_str(), 0) <= W - MARGIN - 104 {
                (t.ok, names)
            } else {
                (t.ok, Fmt::new(format_args!("{c} connectés")))
            }
        }
        (0, s) => (t.warn, Fmt::new(format_args!("{s} en recherche"))),
        (c, s) => (t.warn, Fmt::new(format_args!("{c} OK, {s} en recherche"))),
    };
    row(
        d,
        (&label, "Capteurs"),
        (&value.colored(sensors.0), sensors.1.as_str()),
        176,
        MARGIN,
    )?;
    // 4 pages: a smaller font and tighter separators, cut with "…" if still too long
    let mut names = ui.next_pages().names();
    let room = W - 2 * MARGIN - fonts::ROW.width("Pages", 0) - 10;
    let pages_font = if fonts::ROW.width(names.as_str(), 0) <= room {
        &fonts::ROW
    } else {
        let mut tight = Fmt::new(format_args!(""));
        for (i, name) in names.as_str().split(" · ").enumerate() {
            let sep = if i > 0 { "·" } else { "" };
            tight = Fmt::new(format_args!("{}{sep}{name}", tight.as_str()));
        }
        names = tight;
        &fonts::NOTE
    };
    row(
        d,
        (&label, "Pages"),
        (&t.style(pages_font), fit(pages_font, names.as_str(), room).as_str()),
        203,
        MARGIN,
    )?;
    let mut pending = match v.rides_stored {
        Some(0) => (t.fg, Fmt::new(format_args!("aucune"))),
        Some(1) => (t.warn, Fmt::new(format_args!("1 sortie"))),
        Some(n) => (t.warn, Fmt::new(format_args!("{n} sorties"))),
        None => (t.mute, Fmt::new(format_args!("{NONE}"))),
    };
    // and the room left, in hours of riding: short, in red
    if let Some(h) = v.record_hours {
        let _ = core::fmt::Write::write_fmt(&mut pending.1, format_args!(" · {h} h libres"));
        if h < LOW_RECORD_HOURS {
            pending.0 = t.danger;
        }
    }
    row(
        d,
        (&label, "À envoyer"),
        (&value.colored(pending.0), pending.1.as_str()),
        230,
        MARGIN,
    )
}

pub(super) const MENU_NOTE: (&str, &str) = (
    "Dans l'app :",
    "envoi de la trace, pages de données,\nzones FC et FTP, roue et manivelle,\nréseaux Wi-Fi et serveur de synchro.",
);
pub(super) const PAUSE_MENU_NOTE: (&str, &str) = (
    "",
    "Pour changer de trace, envoie-la depuis\nl'app, même pendant la sortie.",
);

/// A menu: an optional title, the entries (the selected one in blue), a note.
pub(super) fn menu<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    v: &View,
    t: &Theme,
    title: Option<&str>,
    (note_title, note): (&str, &str),
) -> Result<(), D::Error> {
    let (items, sel) = ui.menu(v);
    // six entries fit a little tighter
    let row = if items.iter().flatten().count() > 5 { 41 } else { 43 };
    let mut y = TOP + 1;
    if let Some(title) = title {
        fonts::text(d, &t.label(&fonts::LABEL), title, MARGIN, 47, Align::Left)?;
        y = 62;
    }
    let mut buf = [0; 4];
    let chevron = char_str(icon::CHEVRON, &mut buf);
    for (i, item) in items.iter().flatten().enumerate() {
        // abandoning the ride in the danger colour
        let (bg, fg) = match (i as u8 == sel, *item == Item::Discard) {
            (true, true) => t.kinds[2],
            (true, false) => (t.sel, t.kinds[3].1),
            (false, true) => (t.bg, t.danger),
            (false, false) => (t.bg, t.fg),
        };
        fill(d, rect(0, y, W - 1, y + row - 2), bg)?;
        hline(d, y + row - 1, t)?;
        let s = Style::new(&fonts::MENU, fg, bg);
        let cy = y + row / 2 - 1;
        fonts::text(d, &s, item.label(), MARGIN, cy, Align::Left)?;
        if item.opens() {
            fonts::text(d, &s, chevron, W - MARGIN, cy, Align::Right)?;
        }
        y += row;
    }
    let mut cy = y + 20;
    // four entries leave no room for the note
    if y > 200 {
        return Ok(());
    }
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

/// The logo at power-on: a freewheel sprocket in the route colour ("rouler en liberté"), the name, the motto, the
/// Bluetooth name (which device is which), the version.
pub(super) fn splash<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, t: &Theme) -> Result<(), D::Error> {
    let (cx, cy) = ((W / 2) as f32, 116.0);
    let c = t.route;
    // the teeth, cut out of a full disc like a real freewheel: pointed teeth between wide round seats for the
    // chain's rollers, the flanks opening outwards
    const TEETH: i32 = 20;
    let (root, tip) = (47.0, 56.0);
    let step = core::f32::consts::TAU / TEETH as f32;
    let at = |r: f32, a: f32| Point::new((cx + r * libm::cosf(a)) as i32, (cy + r * libm::sinf(a)) as i32);
    let cut = PrimitiveStyle::with_fill(t.bg);
    disc(d, cx, cy, tip, c, t.bg)?;
    let (seat_r, seat) = (root + 1.5, 4.2);
    // the seats first: the flanks drawn over them clear the outer half of their smoothed edge
    for k in 0..TEETH {
        let a = (k as f32 + 0.5) * step;
        disc(d, cx + seat_r * libm::cosf(a), cy + seat_r * libm::sinf(a), seat, t.bg, c)?;
    }
    for k in 0..TEETH {
        let a = (k as f32 + 0.5) * step;
        let (h0, h1) = (seat / seat_r, 9.4 / (tip + 4.0));
        let (p0, p1) = (at(seat_r, a - h0), at(seat_r, a + h0));
        let (p2, p3) = (at(tip + 4.0, a + h1), at(tip + 4.0, a - h1));
        Triangle::new(p0, p1, p2).into_styled(cut).draw(d)?;
        Triangle::new(p0, p2, p3).into_styled(cut).draw(d)?;
    }
    // the freewheel body: a raised ring (a shade apart, a fine groove around it), its two holes, the bore
    let body = mix(c, t.fg);
    disc(d, cx, cy, 38.0, t.bg, c)?;
    disc(d, cx, cy, 36.8, body, t.bg)?;
    disc(d, cx, cy, 27.5, c, body)?;
    disc(d, cx, cy, 24.0, t.bg, c)?;
    for side in [-1.0, 1.0] {
        disc(d, cx + side * 32.2, cy, 2.3, t.bg, body)?;
    }
    // a route through the bore: an S from a ringed start, bottom left, to the arrival in the route colour, top right
    let path = |u: f32| {
        let x = -13.0 + 26.0 * u + 5.0 * libm::sinf(core::f32::consts::TAU * u);
        let y = 11.0 * libm::cosf(core::f32::consts::PI * u);
        (cx + x, cy + y)
    };
    const SAMPLES: i32 = 24;
    let line = PrimitiveStyle::with_stroke(t.fg, 3);
    let mut last = path(0.0);
    for k in 1..=SAMPLES {
        let p = path(k as f32 / SAMPLES as f32);
        Line::new(Point::new(last.0 as i32, last.1 as i32), Point::new(p.0 as i32, p.1 as i32))
            .into_styled(line)
            .draw(d)?;
        last = p;
    }
    let (sx, sy) = path(0.0);
    disc(d, sx, sy, 4.0, t.fg, t.bg)?;
    disc(d, sx, sy, 2.0, t.bg, t.fg)?;
    let (ex, ey) = path(1.0);
    disc(d, ex, ey, 4.2, c, t.bg)?;
    // the name: Open in the mute colour, the model strong, as one word
    let (open, cc) = ("Open", ui.model());
    let (wo, wc) = (fonts::TITLE.width(open, 0), fonts::TITLE.width(cc, 0));
    let x0 = (W - wo - wc) / 2;
    fonts::text(d, &t.mute(&fonts::TITLE), open, x0, 214, Align::Left)?;
    fonts::text(d, &t.style(&fonts::TITLE), cc, x0 + wo, 214, Align::Left)?;
    fonts::text(d, &t.mute(&fonts::BODY), "Rouler en liberté", W / 2, 246, Align::Center)?;
    fonts::text(d, &t.mute(&fonts::NOTE), ui.ble_name(), W / 2, 278, Align::Center)?;
        fonts::text(d, &t.mute(&fonts::NOTE), crate::VERSION, W / 2, 300, Align::Center)?;
    Ok(())
}

/// The QR code of [`crate::ABOUT_URL`], a row a word, bit `x` the module `x`: `segno.make(ABOUT_URL, error='m')`
/// (Python), version 3, to make again when the address changes.
const ABOUT_QR: [u32; 29] = [
    0x1fd85f7f,
    0x10473b41,
    0x1749a85d,
    0x174ab15d,
    0x1749825d,
    0x10583841,
    0x1fd5557f,
    0x000ad700,
    0x1a4f1aed,
    0x11f95d90,
    0x0d964ff9,
    0x10f4bea8,
    0x068a3ef1,
    0x1c7bf504,
    0x1cca466d,
    0x096dd69b,
    0x0ba044df,
    0x0ea2ba26,
    0x04229471,
    0x055a4780,
    0x07fe217a,
    0x1f1f3900,
    0x0b5b3f7f,
    0x131c9f41,
    0x1df2485d,
    0x13a1f75d,
    0x148e2b5d,
    0x0bd93241,
    0x0870b97f,
];

/// The name, the version and the commit, a QR code to the project's page and its address.
pub(super) fn about<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, t: &Theme) -> Result<(), D::Error> {
    fonts::text(d, &t.label(&fonts::LABEL), "À PROPOS", MARGIN, 47, Align::Left)?;
    // the name like the splash screen, the version on its line
    let (open, cc) = ("Open", ui.model());
    let wo = fonts::TITLE.width(open, 0);
    fonts::text(d, &t.mute(&fonts::TITLE), open, MARGIN, 78, Align::Left)?;
    fonts::text(d, &t.style(&fonts::TITLE), cc, MARGIN + wo, 78, Align::Left)?;
        fonts::text(d, &t.mute(&fonts::BODY), crate::VERSION, W - MARGIN, 78, Align::Right)?;
    let build = Fmt::new(format_args!("Build {}", crate::GIT_SHA));
    fonts::text(d, &t.mute(&fonts::NOTE), build.as_str(), MARGIN, 102, Align::Left)?;
    // dark modules on white whatever the theme, two modules of quiet zone around
    const MODULE: i32 = 4;
    const QUIET: i32 = 2;
    let n = ABOUT_QR.len() as i32;
    let side = (n + 2 * QUIET) * MODULE;
    let (x0, y0) = ((W - side) / 2, 114);
    fill(d, rect(x0, y0, x0 + side - 1, y0 + side - 1), Rgb565::WHITE)?;
    for (y, bits) in (0..).zip(ABOUT_QR) {
        for x in (0..n).filter(|x| bits >> x & 1 == 1) {
            let (px, py) = (x0 + (QUIET + x) * MODULE, y0 + (QUIET + y) * MODULE);
            fill(d, rect(px, py, px + MODULE - 1, py + MODULE - 1), Rgb565::BLACK)?;
        }
    }
    let host = crate::ABOUT_URL.trim_start_matches("https://").trim_end_matches('/');
    fonts::text(d, &t.mute(&fonts::NOTE), host, W / 2, y0 + side + 16, Align::Center)?;
    Ok(())
}

/// A battery's colour, the same in the top bar and on the charging screen: red when low, light blue (the device's key
/// marks) while charging, green when full on the charger; the top bar keeps the mute colour on battery.
pub(super) fn battery_color(pct: u8, plugged: bool, t: &Theme) -> Rgb565 {
    match (pct, plugged) {
        (0..=15, _) => t.danger,
        (100.., true) => t.ok,
        _ => t.key_mark,
    }
}

/// The top bar's battery, icon and percentage, on battery power: amber under 20 %, red under 10 %.
pub(super) fn low_battery(pct: u8, t: &Theme) -> Option<Rgb565> {
    match pct {
        0..10 => Some(t.danger),
        10..20 => Some(t.warn),
        _ => None,
    }
}

/// Halfway between two colours.
pub(super) fn mix(a: Rgb565, b: Rgb565) -> Rgb565 {
    Rgb565::new(
        ((a.r() as u16 + b.r() as u16) / 2) as u8,
        ((a.g() as u16 + b.g() as u16) / 2) as u8,
        ((a.b() as u16 + b.b() as u16) / 2) as u8,
    )
}

/// The charging screen, all of it (no top or key bar): which key switches on, marked on the side of the key, then
/// the level and a big battery filling up and, while charging, the current flowing up into it from the USB port (the
/// bottom edge); centred when there is no current to show.
pub(super) fn charging<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    let pct = v.battery_pct.unwrap_or(0).min(100) as i32;
    let full = v.battery_pct == Some(100);
    let charging = v.plugged && !full;
    let color = battery_color(pct as u8, v.plugged, t);
    // the key: its mark on the edge next to it, what to do
    fill(d, rect(0, 0, TOP_KEY - 1, KEY_MARK - 1), t.key_mark)?;
    let hint = Fmt::new(format_args!("{} Maintenir pour allumer", icon::POWER));
    fonts::text(d, &t.style(&fonts::KEY), hint.as_str(), KEY_PAD, (KEY_MARK + TOP) / 2, Align::Left)?;
    // the block: the level (top at y0), the state, the battery; the cable under it down to the edge
    const BLOCK: i32 = 206;
    // one place whatever the state, room for the current under the battery: nothing moves at full charge
    let y0 = TOP + (H - TOP - BLOCK) / 2 - 10;
    let digits = Fmt::new(format_args!("{pct}"));
    let (dw, pw) = (fonts::HUGE.width(digits.as_str(), 0), fonts::VALUE.width("%", 0));
    let x0 = (W - dw - 6 - pw) / 2;
    let cy = y0 + fonts::HUGE.cap as i32 / 2;
    fonts::text(d, &t.style(&fonts::HUGE), digits.as_str(), x0, cy, Align::Left)?;
    let base = y0 + fonts::HUGE.cap as i32;
    fonts::text(d, &t.style(&fonts::VALUE), "%", x0 + dw + 6, base - fonts::VALUE.cap as i32 / 2, Align::Left)?;
    let (state, c) = match (v.plugged, full) {
        (true, true) => ("Chargé", t.fg),
        (true, false) => ("En charge", t.fg),
        (false, _) => ("Débranché", t.warn),
    };
    fonts::text(d, &t.style(&fonts::BODY_BOLD).colored(c), state, W / 2, y0 + 82, Align::Center)?;
    // the battery: its cap, the body, the inside
    let (left, right, top, bottom) = (90, 150, y0 + BLOCK - 90, y0 + BLOCK);
    let (phase, cycle) = (ui.charge_ms() as i32, crate::CHARGE_CYCLE_MS as i32);
    // the current from the USB port (the bottom edge): dots rising into the battery, drawn first so that they come
    // out from under the edge and go in under the battery's body
    if charging {
        let (from, to) = (H, bottom + 1);
        let gap = 14;
        // anti-aliased discs: they move by a fraction of a pixel
        let mut y = (from + 4) as f32 - (gap * phase) as f32 / cycle as f32;
        while y >= (to - 4) as f32 {
            // brighter near the battery
            let near = (from as f32 - y) * 2.0 >= (from - to) as f32;
            disc(d, (W / 2) as f32 - 0.5, y, 3.5, if near { color } else { mix(color, t.bg) }, t.bg)?;
            y -= gap as f32;
        }
    }
    rounded(d, rect(left + 18, top - 7, right - 18, top + 4), 3, t.fg, t.bg)?;
    rounded(d, rect(left, top, right, bottom), 11, t.fg, t.bg)?;
    rounded(d, rect(left + 5, top + 5, right - 5, bottom - 5), 7, t.bg, t.fg)?;
    let (il, ir, it, ib) = (left + 9, right - 9, top + 9, bottom - 9);
    let h = ib - it + 1;
    // while charging, room is left above the level for the band to show, even near 100 %
    let level = if charging { (h * pct / 100).min(h - 9) } else { h * pct / 100 };
    // charging: a lighter band climbs from the level to the top. Band and level are one rounded shape cut in two
    // colours; the band is never thinner than the corners, so that they stay in it and the level keeps its shape
    let band = if charging { 4 + (h - level - 4) * phase / cycle } else { 0 };
    let shape = rect(il, ib - level - band + 1, ir, ib);
    if band > 0 {
        rounded(&mut d.clipped(&rect(il, it, ir, ib - level)), shape, 4, mix(color, t.bg), t.bg)?;
    }
    if level > 0 {
        rounded(&mut d.clipped(&rect(il, ib - level + 1, ir, ib)), shape, 4, color, t.bg)?;
    }
    // the bolt in the middle, outlined in the background colour: it shows on the fill and on the empty part
    let (bx, by) = ((left + right) / 2, (top + bottom) / 2);
    let bolt = |d: &mut D, dx: i32, dy: i32, c: Rgb565| -> Result<(), D::Error> {
        let p = |x: i32, y: i32| Point::new(bx + x * 2 + dx, by + y * 2 + dy);
        let s = PrimitiveStyle::with_fill(c);
        Triangle::new(p(4, -13), p(-7, 2), p(1, 2)).into_styled(s).draw(d)?;
        Triangle::new(p(-1, -2), p(7, -2), p(-4, 13)).into_styled(s).draw(d)
    };
    for (dx, dy) in [(-2, 0), (2, 0), (0, -2), (0, 2)] {
        bolt(d, dx, dy, t.bg)?;
    }
    bolt(d, 0, 0, if v.plugged { t.fg } else { t.mute })?;
    Ok(())
}

/// A selectable row: its background and text colour.
fn row_colors(t: &Theme, selected: bool) -> (Rgb565, Rgb565) {
    if selected { (t.sel, t.kinds[3].1) } else { (t.bg, t.fg) }
}

/// What a connected sensor reads now, for its row.
fn sensor_value(role: Role, v: &View) -> Option<Fmt> {
    match role {
        Role::HeartRate => v.heart_rate.map(|b| Fmt::new(format_args!("{b} bpm"))),
        Role::SpeedCadence => match (v.cadence_rpm, v.wheel_speed_mm_s) {
            (Some(c), Some(s)) => Some(Fmt::new(format_args!("{c} tr/min, {} km/h", kmh(Some(s)).as_str()))),
            (Some(c), None) => Some(Fmt::new(format_args!("{c} tr/min"))),
            (None, Some(s)) => Some(Fmt::new(format_args!("{} km/h", kmh(Some(s)).as_str()))),
            (None, None) => None,
        },
        Role::Power => v.power_w.map(|w| Fmt::new(format_args!("{w} W"))),
    }
}

const SENSORS_NOTE: &str = "Chercher : réveille d'abord le\ncapteur (roue, pédale, ceinture).";

/// The sensor roles, each with its sensor, its link and its value; the selected one in blue.
pub(super) fn sensors<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    const ROW: i32 = 54;
    fonts::text(d, &t.label(&fonts::LABEL), "CAPTEURS", MARGIN, 47, Align::Left)?;
    let mut buf = [0; 4];
    let chevron = char_str(icon::CHEVRON, &mut buf);
    let mut y = 62;
    for (i, role) in Role::ALL.into_iter().enumerate() {
        let selected = role == ui.role() && !ui.calibrate_row();
        let (bg, fg) = row_colors(t, selected);
        fill(d, rect(0, y, W - 1, y + ROW - 2), bg)?;
        hline(d, y + ROW - 1, t)?;
        let s = Style::new(&fonts::MENU, fg, bg);
        fonts::text(d, &s, role.label(), MARGIN, y + 17, Align::Left)?;
        fonts::text(d, &s, chevron, W - MARGIN, y + ROW / 2 - 1, Align::Right)?;
        let r = &v.sensors.roles[i];
        let name = r.name.as_ref().map_or("capteur", |n| n.as_str());
        let (state, color) = match r.link {
            Link::Unpaired => (Fmt::new(format_args!("Aucun capteur")), t.mute),
            Link::Searching => (Fmt::new(format_args!("{name} : recherche…")), t.warn),
            Link::Connected => match sensor_value(role, v) {
                Some(value) => (Fmt::new(format_args!("{name} · {}", value.as_str())), t.ok),
                None => (Fmt::new(format_args!("{name} : connecté")), t.ok),
            },
        };
        let s = Style::new(&fonts::NOTE, if selected { fg } else { color }, bg);
        fonts::text(d, &s, state.as_str(), MARGIN, y + 38, Align::Left)?;
        y += ROW;
    }
    // the power meter's calibration, in the note's place
    if v.sensors.roles[2].link != Link::Unpaired {
        const CALIBRATE_ROW: i32 = 42;
        let (bg, fg) = row_colors(t, ui.calibrate_row());
        fill(d, rect(0, y, W - 1, y + CALIBRATE_ROW - 2), bg)?;
        hline(d, y + CALIBRATE_ROW - 1, t)?;
        let s = Style::new(&fonts::MENU, fg, bg);
        let cy = y + CALIBRATE_ROW / 2 - 1;
        fonts::text(d, &s, "Calibrer la puissance", MARGIN, cy, Align::Left)?;
        fonts::text(d, &s, chevron, W - MARGIN, cy, Align::Right)?;
        return Ok(());
    }
    note_lines(d, &t.mute(&fonts::NOTE), SENSORS_NOTE, MARGIN, y + 16, 19)
}

/// The power meter's zero offset calibration: what to do before, then how it went.
pub(super) fn calibrate<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    use crate::Calibration;
    fonts::text(d, &t.label(&fonts::LABEL), "CALIBRER LA PUISSANCE", MARGIN, 47, Align::Left)?;
    let todo = "Vélo à l'arrêt, pieds au\nsol, manivelle en bas.\nNe touche plus les\npédales pendant ~10 s.";
    note_lines(d, &t.style(&fonts::BODY), todo, MARGIN, 82, 22)?;
    let when = "Avant chaque sortie, et quand\nla température a bien changé.";
    note_lines(d, &t.mute(&fonts::NOTE), when, MARGIN, 172, 19)?;
    let body = t.style(&fonts::BODY_BOLD);
    let (mark, text, color) = match v.sensors.calibration {
        Calibration::None => return Ok(()),
        Calibration::Running => (icon::DOT, Fmt::new(format_args!("Calibration…")), t.warn),
        Calibration::Done(offset) => (icon::CHECK, Fmt::new(format_args!("Réussie · offset {offset}")), t.ok),
        Calibration::Failed => (icon::CROSS, Fmt::new(format_args!("Échec : réessaie")), t.danger),
        Calibration::Unsupported => (icon::CROSS, Fmt::new(format_args!("Pas prise en charge")), t.danger),
        Calibration::NotConnected => (icon::CROSS, Fmt::new(format_args!("Capteur non connecté")), t.danger),
        Calibration::Timeout => (icon::CROSS, Fmt::new(format_args!("Pas de réponse")), t.danger),
    };
    bullet(d, &body.colored(color), mark, text.as_str(), 236, 22)
}

const SCAN_NOTE: &str = "Recherche en cours…\nRéveille le capteur : tourne la roue,\npédale ou mets la ceinture.";

/// The sensors found around for the role being paired, closest first, then "none" if it has one.
pub(super) fn sensor_scan<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    const ROW: i32 = 42;
    let role = ui.role();
    let title = Fmt::new(format_args!("RECHERCHE · {}", role.title()));
    fonts::text(d, &t.label(&fonts::LABEL), title.as_str(), MARGIN, 47, Align::Left)?;
    let mut buf = [0; 4];
    let check = char_str(icon::CHECK, &mut buf);
    let mut y = 62;
    let mut i = 0u8;
    for f in v.sensors.found.iter().flatten() {
        let (bg, fg) = row_colors(t, i == ui.scan_item());
        fill(d, rect(0, y, W - 1, y + ROW - 2), bg)?;
        hline(d, y + ROW - 1, t)?;
        let cy = y + ROW / 2 - 1;
        let s = Style::new(&fonts::MENU, fg, bg);
        let mut left = MARGIN;
        if f.paired {
            fonts::text(d, &s, check, left, cy, Align::Left)?;
            left += fonts::MENU.width(check, 0) + 6;
        }
        let signal = Fmt::new(format_args!("{} dBm", f.rssi_dbm));
        let n = Style::new(&fonts::NOTE, if i == ui.scan_item() { fg } else { t.mute }, bg);
        fonts::text(d, &n, signal.as_str(), W - MARGIN, cy, Align::Right)?;
        fonts::text(d, &s, f.name.as_str(), left, cy, Align::Left)?;
        y += ROW;
        i += 1;
    }
    if v.sensors.roles[role as usize].link != Link::Unpaired {
        let (bg, fg) = row_colors(t, i == ui.scan_item());
        fill(d, rect(0, y, W - 1, y + ROW - 2), bg)?;
        hline(d, y + ROW - 1, t)?;
        let s = Style::new(&fonts::MENU, fg, bg);
        fonts::text(d, &s, "Aucun (oublier)", MARGIN, y + ROW / 2 - 1, Align::Left)?;
        y += ROW;
    }
    if i == 0 {
        note_lines(d, &t.mute(&fonts::NOTE), SCAN_NOTE, MARGIN, y + 20, 19)?;
    }
    Ok(())
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

pub(super) const PAUSED_NOTE: &str = "Pause longue ? Menu › Éteindre :\nla sortie reprend au rallumage.";
pub(super) const RESUME_NOTE: &str = "Le temps éteint ne compte pas\ndans la durée.";

/// The pause, and the paused ride found at power-on.
pub(super) fn paused<D: DrawTarget<Color = Rgb565>>(
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
    // why the floppy disk of the top bar is red, in place of the note
    if v.sparse {
        let c = t.danger;
        chrome::floppy(d, m + 13, 246, c, t.bg)?;
        fonts::text(d, &t.style(&fonts::NOTE_BOLD).colored(c), "Mémoire presque pleine", m + 20, 246, Align::Left)?;
        let more = match v.record_hours {
            Some(h) => Fmt::new(format_args!("1 point toutes les 5 s · {h} h libres")),
            None => Fmt::new(format_args!("1 point toutes les 5 s")),
        };
        return fonts::text(d, &t.mute(&fonts::NOTE), more.as_str(), m + 20, 264, Align::Left).map(|_| ());
    }
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

pub(super) fn confirm_stop<D: DrawTarget<Color = Rgb565>>(
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

/// "0,0 km · 1:12"
fn ride_summary(v: &View) -> Fmt {
    Fmt::new(format_args!("{} km · {}", km(v.ride.distance_cm).as_str(), duration(v.ride.timer_s).as_str()))
}

pub(super) fn confirm_empty<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    // grey: nothing to lose
    fill(d, rect(0, TOP + 1, W - 1, TOP + 6), t.mute)?;
    fonts::text(d, &t.style(&fonts::HEADING), "Sortie vide", 14, 63, Align::Left)?;
    fonts::text(d, &t.style(&fonts::BODY_BOLD), ride_summary(v).as_str(), 14, 100, Align::Left)?;
    note_lines(d, &t.style(&fonts::BODY), "Rien à garder : elle est\neffacée, pas enregistrée.", 14, 132, 22)?;
    note_lines(d, &t.mute(&fonts::NOTE), "Pour la garder quand même :\nEnregistrer.", 14, 196, 19)
}

pub(super) fn confirm_discard<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    fill(d, rect(0, TOP + 1, W - 1, TOP + 6), t.danger)?;
    fonts::text(d, &t.style(&fonts::HEADING), "Abandonner la sortie ?", 14, 63, Align::Left)?;
    let lost = Fmt::new(format_args!("{}\nne sera pas enregistrée", ride_summary(v).as_str()));
    bullet(d, &t.style(&fonts::BODY), icon::CROSS, lost.as_str(), 96, 22)?;
    bullet(d, &t.style(&fonts::BODY_BOLD), icon::CROSS, "Impossible à reprendre", 150, 22)
}

pub(super) fn confirm_discard_again<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    fill(d, rect(0, TOP + 1, W - 1, TOP + 6), t.danger)?;
    fonts::text(d, &t.style(&fonts::HEADING).colored(t.danger), "Vraiment effacer ?", 14, 63, Align::Left)?;
    fonts::text(d, &t.style(&fonts::BODY_BOLD), ride_summary(v).as_str(), 14, 100, Align::Left)?;
    fonts::text(d, &t.style(&fonts::BODY), "Elle sera perdue pour de bon.", 14, 124, Align::Left)?;
    let note = "Attention, les touches changent :\nEffacer à gauche, Garder à droite.";
    note_lines(d, &t.mute(&fonts::NOTE), note, 14, 196, 19)
}

const WEEKDAYS: [&str; 7] = ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."];
const MONTHS: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc.",
];

/// "sam. 12 sept."
fn ride_date(c: &RideCard) -> Fmt {
    let weekday = WEEKDAYS.get(c.weekday as usize).copied().unwrap_or("");
    let month = MONTHS.get(c.month.wrapping_sub(1) as usize).copied().unwrap_or("");
    Fmt::new(format_args!("{weekday} {} {month}", c.day))
}

/// "08:47 · 3 h 10 · 74,2 km" ("45 min" under an hour)
fn ride_line(c: &RideCard) -> Fmt {
    let (h, min) = (c.timer_s / 3600, c.timer_s / 60 % 60);
    let time = if h > 0 { Fmt::new(format_args!("{h} h {min:02}")) } else { Fmt::new(format_args!("{min} min")) };
    Fmt::new(format_args!("{:02}:{:02} · {} · {} km", c.hour, c.minute, time.as_str(), km(c.distance_cm).as_str()))
}

/// Whether the app has the ride: a ride it does not have is lost once deleted.
fn sent_bullet<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    c: &RideCard,
    cy: i32,
    t: &Theme,
) -> Result<(), D::Error> {
    if c.sent {
        bullet(d, &t.style(&fonts::BODY).colored(t.ok), icon::CHECK, "Envoyée à l'appli", cy, 22)
    } else {
        bullet(d, &t.style(&fonts::BODY_BOLD).colored(t.warn), icon::CROSS, "Pas encore envoyée", cy, 22)
    }
}

/// The finished rides, one at a time (the ones the app has first, the oldest first).
pub(super) fn rides<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    let r = &v.rides;
    let title = if ui.making_room() { "MÉMOIRE PLEINE" } else { "SORTIES ENREGISTRÉES" };
    fonts::text(d, &t.label(&fonts::LABEL), title, MARGIN, 47, Align::Left)?;
    let Some(card) = r.card else {
        return fonts::text(d, &t.style(&fonts::BODY), "Aucune sortie enregistrée", MARGIN, 90, Align::Left).map(|_| ());
    };
    let pos = Fmt::new(format_args!("{} / {}", ui.ride_item() + 1, r.count));
    fonts::text(d, &t.mute(&fonts::LABEL), pos.as_str(), W - MARGIN, 47, Align::Right)?;
    fonts::text(d, &t.style(&fonts::HEADING), ride_date(&card).as_str(), MARGIN, 85, Align::Left)?;
    fonts::text(d, &t.style(&fonts::BODY), ride_line(&card).as_str(), MARGIN, 117, Align::Left)?;
    sent_bullet(d, &card, 149, t)?;
    let note = match (ui.making_room(), r.can_start) {
        (true, true) => "Place suffisante : tu peux démarrer.",
        (true, false) => "Pour démarrer, supprime une sortie.\nCelles déjà dans l'appli d'abord.",
        _ => "La plus récente d'abord.",
    };
    let color = if ui.making_room() && r.can_start { t.ok } else { t.mute };
    note_lines(d, &t.mute(&fonts::NOTE).colored(color), note, MARGIN, 190, 19)?;
    if let Some(h) = v.record_hours {
        let free = Fmt::new(format_args!("{h} h d'enregistrement libres"));
        fonts::text(d, &t.mute(&fonts::NOTE), free.as_str(), MARGIN, 240, Align::Left)?;
    }
    Ok(())
}

pub(super) fn confirm_delete<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    fill(d, rect(0, TOP + 1, W - 1, TOP + 6), t.danger)?;
    fonts::text(d, &t.style(&fonts::HEADING), "Supprimer la sortie ?", 14, 63, Align::Left)?;
    let Some(card) = v.rides.card else { return Ok(()) };
    let what = Fmt::new(format_args!("{}\n{}", ride_date(&card).as_str(), ride_line(&card).as_str()));
    fonts::lines(d, &t.style(&fonts::BODY), what.as_str(), 14, 100, 24, Align::Left)?;
    sent_bullet(d, &card, 160, t)?;
    if !card.sent {
        note_lines(d, &t.mute(&fonts::NOTE), "Elle sera perdue : l'appli\nne l'a pas encore.", 14, 196, 19)?;
    }
    Ok(())
}

pub(super) fn saved<D: DrawTarget<Color = Rgb565>>(d: &mut D, v: &View, t: &Theme) -> Result<(), D::Error> {
    // saving takes a few seconds: said, not left to guess
    let (title, color) = if v.saving {
        (Fmt::new(format_args!("Enregistrement…")), t.mute)
    } else {
        (Fmt::new(format_args!("{} Sortie enregistrée", icon::CHECK)), t.ok)
    };
    fonts::text(
        d,
        &t.style(&fonts::HEADING).colored(color),
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
    ride::profile(d, rect(14, 166, 226, 200), v.whole.points(), false, t)?;
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

