use super::*;
use super::primitives::{disc, fill, hline, rounded};

// --- banner ------------------------------------------------------------------------------------------------------

pub(super) fn banner<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    b: &crate::Banner,
    t: &Theme,
) -> Result<(), D::Error> {
    let area = rect(6, 221, 233, 272);
    rounded(d, area, 6, t.banner_bg, t.bg)?;
    let mut arrow = [0; 4];
    let (circle, mark, mark_fg) = match b.kind {
        BannerKind::Info => (t.info, "i", rgb(0xFFFFFF)),
        // something coming in: the arrow, not the information's "i"
        BannerKind::Progress => (t.info, &*icon::DOWN.encode_utf8(&mut arrow), rgb(0xFFFFFF)),
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
    // only a progress has a bar: what is done, on its track
    if b.kind == BannerKind::Progress {
        let full = area.size.width as i32 - 12;
        let w = full * i32::from(b.bar_permille) / 1000;
        fill(d, rect(12, 266, 12 + full - 1, 269), t.rule)?;
        if w > 0 {
            fill(d, rect(12, 266, 12 + w - 1, 269), t.info)?;
        }
    }
    Ok(())
}

// --- bars --------------------------------------------------------------------------------------------------------

pub(super) fn top_bar<D: DrawTarget<Color = Rgb565>>(
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
    // on the map, the scale bar after its length: |____|
    if let Some(px) = ui.map_scale(v) {
        debug_assert!(fonts::KEY.width(hint.text, 0) + 4 + px + 2 * KEY_PAD <= TOP_KEY, "scale too wide");
        let (x1, y) = (TOP_KEY - KEY_PAD - 1, (KEY_MARK + TOP) / 2 + 5);
        let x0 = x1 - px + 1;
        fill(d, rect(x0, y - 1, x1, y), fg)?;
        fill(d, rect(x0, y - 7, x0 + 1, y), fg)?;
        fill(d, rect(x1 - 1, y - 7, x1, y), fg)?;
    }
    let cy = TOP / 2;
    let bar = t.mute(&fonts::BAR);
    // the battery icon 3 px before "100%" (full, written without its space to stay close): it stays put from 100 to
    // 10 %, 7 px before two digits; closer to the edge than the keys' labels, room for the distance off the route
    let right = W - BAR_PAD - fonts::BAR.width("100%", 0) - 3;
    let left = right - BATTERY_W;
    if let Some(p) = v.battery_pct {
        let pct = if p >= 100 { Fmt::new(format_args!("100%")) } else { Fmt::new(format_args!("{p} %")) };
        let text = match super::screens::low_battery(p, t) {
            Some(c) if !v.plugged => bar.colored(c),
            _ => bar,
        };
        fonts::value(d, &text, pct.as_str(), "", W - BAR_PAD, cy, Align::Right)?;
        battery(d, right, cy, p, v.plugged, t)?;
        // plugged: a bolt over the battery's body
        if v.plugged {
            small_bolt(d, right - 3 - (BATTERY_W - 3) / 2, cy)?;
        }
    }
    // the ride is recorded: a floppy disk before the battery, in its grey; red when the storage is nearly full (a
    // point every 5 s, the pause screen says so) or a write failed (a banner says so)
    let recording = matches!(ui.screen(), Screen::Ride | Screen::Paused);
    if recording {
        floppy(d, left - 6, cy, record_color(v, t), t.bg)?;
    }
    let rune_right = if recording { left - 6 - FLOPPY_W - 4 } else { left - 6 };
    // the clock in the middle of the room between the key and the rune before the floppy disk, whether they show or
    // not: it never moves
    let mid = (TOP_KEY + left - 6 - FLOPPY_W - 4 - RUNE_W) / 2;
    // where the text in the middle ends, when it is not centred
    let mut taken = TOP_KEY;
    if ui.gps_warning() {
        let gps = Fmt::new(format_args!("GPS {}", icon::CROSS));
        fonts::text(
            d,
            &bar.colored(t.warn),
            gps.as_str(),
            mid,
            cy,
            Align::Center,
        )?;
    } else if let Some(dm) = ui.off_route_dm() {
        // after the key (wider than the clock), two figures and the unit against them: room for the rune
        let off = match dm {
            ..1_000 => Fmt::new(format_args!("{} {}m", icon::OFF_ROUTE, dm / 10)),
            ..100_000 => Fmt::new(format_args!("{} {},{}km", icon::OFF_ROUTE, dm / 10_000, dm / 1_000 % 10)),
            _ => Fmt::new(format_args!("{} {}km", icon::OFF_ROUTE, dm / 10_000)),
        };
        let ink = fonts::text(
            d,
            &bar.colored(t.warn),
            off.as_str(),
            TOP_KEY + 3,
            cy,
            Align::Left,
        )?;
        taken = ink.top_left.x + ink.size.width as i32;
    } else {
        fonts::value(
            d,
            &bar,
            clock(v.clock).as_str(),
            "00:00",
            mid,
            cy,
            Align::Center,
        )?;
    }
    // the phone's app is connected: the Bluetooth rune before the floppy disk, or in its place; not over a long
    // distance off the route
    if v.phone && taken + 3 <= rune_right - RUNE_W {
        rune(d, rune_right, cy, t.mute)?;
    }
    hline(d, TOP, t)
}

const BATTERY_W: i32 = 18;
const FLOPPY_W: i32 = 14;
const RUNE_W: i32 = 8;

fn record_color(v: &View, t: &Theme) -> Rgb565 {
    if v.sparse || v.write_failure.is_some() { t.danger } else { t.mute }
}

/// A floppy disk 14 × 14 px, right edge at `right`, centred on `cy`: its top right corner cut, the shutter at the
/// top, the label at the bottom.
pub(super) fn floppy<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    right: i32,
    cy: i32,
    c: Rgb565,
    bg: Rgb565,
) -> Result<(), D::Error> {
    let (l, top, bottom) = (right - 13, cy - 7, cy + 6);
    fill(d, rect(l, top, right - 3, bottom), c)?;
    fill(d, rect(right - 2, top + 2, right, bottom), c)?;
    fill(d, rect(right - 2, top + 1, right - 2, top + 1), c)?;
    // the shutter, and its window
    fill(d, rect(l + 3, top, right - 4, top + 4), bg)?;
    fill(d, rect(right - 7, top + 1, right - 6, top + 3), c)?;
    // the label
    fill(d, rect(l + 2, top + 7, right - 2, bottom - 1), bg)?;
    Ok(())
}

/// The Bluetooth rune, a row a byte (bit 7: the left column), its stem 2 px wide.
const RUNE: [u8; 13] = [
    0b00011000, 0b00011100, 0b00011010, 0b10011001, 0b01011010, 0b00111100, 0b00011000, 0b00111100, 0b01011010,
    0b10011001, 0b00011010, 0b00011100, 0b00011000,
];

/// The Bluetooth rune 8 × 13 px, right edge at `right`, centred on `cy`.
fn rune<D: DrawTarget<Color = Rgb565>>(d: &mut D, right: i32, cy: i32, c: Rgb565) -> Result<(), D::Error> {
    let (x0, y0) = (right - 7, cy - 6);
    for (y, row) in RUNE.iter().enumerate() {
        for x in 0..8 {
            if row & (0x80 >> x) != 0 {
                fill(d, rect(x0 + x, y0 + y as i32, x0 + x, y0 + y as i32), c)?;
            }
        }
    }
    Ok(())
}

/// Battery outline with its level, right edge at `right`, centred on `cy`.
fn battery<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    right: i32,
    cy: i32,
    pct: u8,
    plugged: bool,
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
    let c = match super::screens::low_battery(pct, t) {
        _ if plugged => super::screens::battery_color(pct, plugged, t),
        Some(c) => c,
        None => t.mute,
    };
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

/// The bolt over the top bar's battery, a row a byte (bit 5: the left column).
const SMALL_BOLT: [u8; 11] = [
    0b000001, 0b000011, 0b000110, 0b001110, 0b011100, 0b111111, 0b001110, 0b011100, 0b011000, 0b110000, 0b100000,
];

/// The bolt over the top bar's battery, 6 × 11 px centred on (`cx`, `cy`): black outlined in white, day or night.
fn small_bolt<D: DrawTarget<Color = Rgb565>>(d: &mut D, cx: i32, cy: i32) -> Result<(), D::Error> {
    let on = |x: i32, y: i32| {
        (0..6).contains(&x) && (0..11).contains(&y) && SMALL_BOLT[y as usize] & (0b100000 >> x) != 0
    };
    let (x0, y0) = (cx - 3, cy - 5);
    for y in -1..12 {
        for x in -1..7 {
            let c = if on(x, y) {
                DAY.fg
            } else if on(x - 1, y) || on(x + 1, y) || on(x, y - 1) || on(x, y + 1) {
                DAY.bg
            } else {
                continue;
            };
            fill(d, rect(x0 + x, y0 + y, x0 + x, y0 + y), c)?;
        }
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

pub(super) fn bottom_bar<D: DrawTarget<Color = Rgb565>>(
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

