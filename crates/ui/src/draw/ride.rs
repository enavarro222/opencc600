use super::*;
use super::primitives::{disc, fill, hline, vline};

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

/// The top of the speed, power and heart rate pages ends here: four small cells under it, in two rows.
const CELLS: i32 = 170;

/// A small cell under the top: label style, label, value, the value's smallest slot, and a zone badge after the
/// value (zone, how many zones).
type Small = (Style, &'static str, Fmt, &'static str, Option<(u8, u8)>);

/// The four small cells under the top of a page, two rows.
fn four<D: DrawTarget<Color = Rgb565>>(d: &mut D, t: &Theme, cells: &[Small; 4]) -> Result<(), D::Error> {
    let mid = (CELLS + BOTTOM) / 2;
    hline(d, CELLS, t)?;
    hline(d, mid, t)?;
    vline(d, W / 2 - 1, CELLS + 1, BOTTOM - 1, t)?;
    let value = t.style(&fonts::VALUE);
    for (i, (label, text, val, min, zone)) in cells.iter().enumerate() {
        let (y0, y1) = if i < 2 { (CELLS + 1, mid - 1) } else { (mid + 1, BOTTOM - 1) };
        let (x0, x1) = if i % 2 == 0 { (0, W / 2 - 2) } else { (W / 2, W - 1) };
        let Some((z, count)) = *zone else {
            cell(d, rect(x0, y0, x1, y1), (label, text), (&value, val.as_str(), min), 8)?;
            continue;
        };
        // the value moved left by half the badge
        let cx = (x0 + x1) / 2;
        let (lc, vc) = (label.font.cap as i32, value.font.cap as i32);
        let top = y0 + (y1 - y0 + 1 - (lc + 8 + vc) + 1) / 2;
        fonts::text(d, label, text, cx, top + lc / 2, Align::Center)?;
        let vy = top + lc + 8 + vc / 2;
        let half = fonts::VALUE.width(min, 0) / 2;
        fonts::value(d, &value, val.as_str(), min, cx - 15, vy, Align::Center)?;
        badge(d, z, count, cx - 15 + half + 5, vy)?;
    }
    Ok(())
}

/// "Z3" in the zone's colour, from `x`, centred on `cy`.
fn badge<D: DrawTarget<Color = Rgb565>>(d: &mut D, zone: u8, count: u8, x: i32, cy: i32) -> Result<(), D::Error> {
    let (bg, fg) = zone_colour(zone, count);
    let r = rect(x, cy - 9, x + 25, cy + 8);
    RoundedRectangle::with_equal_corners(r, Size::new(3, 3))
        .into_styled(PrimitiveStyle::with_fill(bg))
        .draw(d)?;
    let z = Fmt::new(format_args!("Z{zone}"));
    fonts::text(d, &Style::new(&fonts::NOTE_BOLD, fg, bg), z.as_str(), x + 13, cy, Align::Center)?;
    Ok(())
}

/// The zone colours, grey to purple, and the text on each.
const ZONES: [(Rgb565, Rgb565); 7] = {
    let (light, dark) = (rgb(0xFFFFFF), rgb(0x111111));
    [
        (rgb(0x8E969E), light),
        (rgb(0x2F7BE8), light),
        (rgb(0x1F9A47), light),
        (rgb(0xF2B705), dark),
        (rgb(0xF57C00), dark),
        (rgb(0xD92B2B), light),
        (rgb(0x7C3AAD), light),
    ]
};

/// The colour of `zone` (1 the first) of `count`: 7 zones take them all, fewer leave some out (6: no purple, 5: no
/// yellow either...).
fn zone_colour(zone: u8, count: u8) -> (Rgb565, Rgb565) {
    const PICK: [&[usize]; 6] = [
        &[2, 5],
        &[2, 3, 5],
        &[0, 2, 4, 5],
        &[0, 1, 2, 4, 5],
        &[0, 1, 2, 3, 4, 5],
        &[0, 1, 2, 3, 4, 5, 6],
    ];
    let pick = PICK[(count.clamp(2, 7) - 2) as usize];
    ZONES[pick[(zone.max(1) as usize - 1).min(pick.len() - 1)]]
}

/// The speed gauge's full scale: the arc is full from 50 km/h.
const GAUGE_MM_S: u32 = 13_889;

/// The speed gauge: an arc from 0 to [`GAUGE_MM_S`], the speed in the `me` colour over the track, a radial tick at
/// the average; ends rounded, edges anti-aliased.
fn gauge<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    speed: Option<u32>,
    avg: Option<u32>,
) -> Result<(), D::Error> {
    const CX: f32 = 120.0;
    const CY: f32 = 136.0;
    const R: f32 = 72.0;
    // half the arc's width, half the tick's length and width
    const HALF: f32 = 5.0;
    const TICK: f32 = 9.0;
    const TICK_HALF: f32 = 1.5;
    // from the left, clockwise, degrees
    const START: f32 = 200.0;
    const SWEEP: f32 = 220.0;
    let deg = |v: u32| SWEEP * (v as f32 / GAUGE_MM_S as f32).min(1.0);
    let fill = speed.filter(|&s| s > 0).map(deg);
    let tick = avg.map(deg);
    // the centre of the arc's end at `along` degrees from its start
    let end = |along: f32| {
        let a = (START - along).to_radians();
        (CX + R * libm::cosf(a), CY - R * libm::sinf(a))
    };
    let cover = |v: f32| v.clamp(0.0, 1.0);
    // an arc from the start to `sweep`, at a pixel `dist` from the centre, `along` degrees from the start
    let band = |sweep: f32, (px, py): (f32, f32), dist: f32, along: f32| {
        if along <= sweep {
            return cover(HALF + 0.5 - libm::fabsf(dist - R));
        }
        let cap = |(ex, ey): (f32, f32)| cover(HALF + 0.5 - libm::sqrtf((px - ex) * (px - ex) + (py - ey) * (py - ey)));
        cap(end(0.0)).max(cap(end(sweep)))
    };
    let alpha = |c: f32| (c * 15.0 + 0.5) as u8;
    let reach = R + TICK + 1.0;
    let hole = R - TICK - 1.0;
    for y in (CY - reach) as i32..=(CY + reach) as i32 {
        let dy = y as f32 + 0.5 - CY;
        let span = reach * reach - dy * dy;
        if span < 0.0 {
            continue;
        }
        let half = libm::sqrtf(span);
        for x in (CX - half) as i32..=(CX + half) as i32 {
            let dx = x as f32 + 0.5 - CX;
            let dist2 = dx * dx + dy * dy;
            if dist2 < hole * hole || dist2 > reach * reach {
                continue;
            }
            let dist = libm::sqrtf(dist2);
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            // atan2 is in -180..180: START - it in 20..380
            let along = START - libm::atan2f(-dy, dx).to_degrees();
            let along = if along >= 360.0 { along - 360.0 } else { along };
            let track = band(SWEEP, p, dist, along);
            let speed = fill.map_or(0.0, |f| band(f, p, dist, along));
            let mark = tick.map_or(0.0, |a| {
                let across = dist * libm::sinf((along - a).to_radians());
                cover(TICK_HALF + 0.5 - libm::fabsf(across)) * cover(TICK + 0.5 - libm::fabsf(dist - R))
                    * if libm::fabsf(along - a) < 90.0 { 1.0 } else { 0.0 }
            });
            if track + speed + mark == 0.0 {
                continue;
            }
            let c = fonts::blend(t.bg, t.profile, alpha(track));
            let c = fonts::blend(c, t.me, alpha(speed));
            let c = fonts::blend(c, t.fg, alpha(mark));
            Pixel(Point::new(x, y), c).draw(d)?;
        }
    }
    Ok(())
}

/// A big value and its unit after it, the pair centred, on one baseline; `cy` the value's capitals' centre.
fn value_unit<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    (value, v): (&Style, &str),
    (unit, u): (&Style, &str),
    cy: i32,
) -> Result<(), D::Error> {
    const GAP: i32 = 6;
    let (wv, wu) = (value.font.width(v, 0), unit.font.width(u, 0));
    let x0 = W / 2 - (wv + GAP + wu) / 2;
    fonts::text(d, value, v, x0, cy, Align::Left)?;
    let baseline = cy + (value.font.cap as i32 + 1) / 2;
    fonts::text(d, unit, u, x0 + wv + GAP, baseline - (unit.font.cap as i32 + 1) / 2, Align::Left)?;
    Ok(())
}

/// What the top of the power and heart rate pages shows: label, value, unit, the reference ("FTP", "FC MAX"), and
/// the hint when the reference is not set.
struct Top {
    label: &'static str,
    value: Option<u16>,
    unit: &'static str,
    reference: (&'static str, u16),
    zones: crate::Zones,
    zone: Option<u8>,
    hint: &'static str,
}

/// The top of the power and heart rate pages: in the colour of the zone, its number and range, the percentage of
/// the reference, the value, a ruler of the zones; without a zone, the value alone on the page's background.
fn zone_top<D: DrawTarget<Color = Rgb565>>(d: &mut D, t: &Theme, top: &Top) -> Result<(), D::Error> {
    let v = int(top.value);
    let (name, reference) = top.reference;
    let unit_font = &fonts::HEADING_SMALL;
    let (Some(z), Some(value)) = (top.zone, top.value) else {
        fonts::text(d, &t.label(&fonts::LABEL), top.label, W / 2, 52, Align::Center)?;
        value_unit(d, (&t.style(&fonts::HUGE), v.as_str()), (&t.style(unit_font), top.unit), 106)?;
        if reference == 0 {
            fonts::text(d, &t.mute(&fonts::NOTE), top.hint, W / 2, 156, Align::Center)?;
        }
        return Ok(());
    };
    let count = top.zones.count();
    let (bg, fg) = zone_colour(z, count);
    fill(d, rect(0, TOP + 1, W - 1, CELLS - 1), bg)?;
    let on = |font: &'static Font| Style::new(font, fg, bg);
    let zone = Fmt::new(format_args!("Z{z}"));
    fonts::text(d, &on(&fonts::TITLE), zone.as_str(), MARGIN, 50, Align::Left)?;
    // the zone's range: "< 127", "208–241", "> 345"
    let unit = if top.unit == "W" { "W" } else { "BPM" };
    let range = match (z.checked_sub(1).and_then(|p| top.zones.top(p, reference)), top.zones.top(z, reference)) {
        (None, Some(hi)) => Fmt::new(format_args!("< {} {unit}", hi + 1)),
        (Some(lo), Some(hi)) => Fmt::new(format_args!("{}–{hi} {unit}", lo + 1)),
        (Some(lo), None) => Fmt::new(format_args!("> {lo} {unit}")),
        (None, None) => Fmt::new(format_args!("")),
    };
    fonts::text(d, &on(&fonts::LABEL_SMALL).spaced(60), range.as_str(), MARGIN, 74, Align::Left)?;
    fonts::text(d, &on(&fonts::LABEL_SMALL).spaced(60), top.label, W - MARGIN, 44, Align::Right)?;
    let pct = Fmt::new(format_args!("{} % {name}", value as u32 * 100 / reference as u32));
    fonts::text(d, &on(&fonts::HEADING_SMALL), pct.as_str(), W - MARGIN, 64, Align::Right)?;
    value_unit(d, (&on(&fonts::HUGE), v.as_str()), (&on(unit_font), top.unit), 116)?;
    // the ruler: a segment a zone, this one filled
    let (x0, x1, gap) = (MARGIN, W - MARGIN, 2);
    let n = count as i32;
    let off = fonts::blend(bg, fg, 5);
    for i in 0..n {
        let a = x0 + (x1 - x0 + gap) * i / n;
        let b = x0 + (x1 - x0 + gap) * (i + 1) / n - gap;
        fill(d, rect(a, 155, b - 1, 162), if i + 1 == z as i32 { fg } else { off })?;
    }
    Ok(())
}

/// Whether the role has a sensor paired.
fn paired(v: &View, role: Role) -> bool {
    v.sensors.roles[Role::ALL.iter().position(|&r| r == role).unwrap_or(0)].name.is_some()
}

/// The heart rate cell, with its zone badge.
fn heart_cell(ui: &Ui, v: &View, t: &Theme) -> Small {
    let zone = ui.hr_zone().map(|z| (z, v.hr_zones.count()));
    (t.label(&fonts::LABEL_SMALL), "FC BPM", int(v.heart_rate), "000", zone)
}

/// Page Essential: the speed on a gauge, the average and max speeds in the corners, then time, distance, D+ and
/// heart rate (else cadence, else the time since the start).
pub(super) fn ride_essential<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    ui: &Ui,
    v: &View,
    t: &Theme,
) -> Result<(), D::Error> {
    let avg = v.ride.avg_speed_mm_s();
    gauge(d, t, v.speed_mm_s, avg)?;
    // the average marked as on the gauge, the max
    let small = t.label(&fonts::LABEL_SMALL);
    let corner = t.style(&fonts::HEADING);
    fill(d, rect(MARGIN, 38, MARGIN + 2, 47), t.fg)?;
    fonts::text(d, &small, "MOY.", MARGIN + 7, 43, Align::Left)?;
    fonts::text(d, &corner, kmh(avg).as_str(), MARGIN, 62, Align::Left)?;
    fonts::text(d, &small, "MAX.", W - MARGIN, 43, Align::Right)?;
    let max = (v.ride.max_speed_mm_s > 0).then_some(v.ride.max_speed_mm_s);
    fonts::text(d, &corner, kmh(max).as_str(), W - MARGIN, 62, Align::Right)?;
    fonts::text(d, &t.label(&fonts::LABEL_SMALL).colored(t.me), "VITESSE KM/H", W / 2, 94, Align::Center)?;
    fonts::value(d, &t.style(&fonts::SPEED), kmh(v.speed_mm_s).as_str(), SPEED, W / 2, 129, Align::Center)?;
    let last = if v.heart_rate.is_some() || paired(v, Role::HeartRate) {
        heart_cell(ui, v, t)
    } else if v.cadence_rpm.is_some() || paired(v, Role::SpeedCadence) {
        (small, "CADENCE", int(v.cadence_rpm), "00", None)
    } else {
        (small, "TEMPS TOTAL", duration(v.ride.elapsed_s), "", None)
    };
    let cells = [
        (small, "EN MOUVEMENT", duration(v.ride.timer_s), "", None),
        (small, "DISTANCE KM", km(v.ride.distance_cm), "00,0", None),
        (small, "D+ M", int(Some(v.ride.ascent_m)), "000", None),
        last,
    ];
    four(d, t, &cells)
}

/// Page Effort (docs/PRODUCT.md), with a power meter paired: 3 s power big in the colour of its zone, then by
/// priority average power, heart rate, cadence, speed, time. What shows follows the sensors paired, not their
/// values: a sensor lost keeps its cell with "–", the layout never moves.
pub(super) fn ride_effort<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    let small = t.label(&fonts::LABEL_SMALL);
    let candidates = [
        Some((small, "MOY. W", int(v.ride.avg_power_w), "000", None)),
        paired(v, Role::HeartRate).then(|| heart_cell(ui, v, t)),
        // from the power meter, else a cadence sensor
        Some((small, "CADENCE", int(v.cadence_rpm), "00", None)),
        Some((small, "VITESSE KM/H", kmh(v.speed_mm_s), SPEED, None)),
        Some((small, "EN MOUVEMENT", duration(v.ride.timer_s), "", None)),
    ];
    let mut shown = candidates.into_iter().flatten();
    let cells = core::array::from_fn(|_| shown.next().unwrap_or((small, "", Fmt::new(format_args!("")), "", None)));
    zone_top(
        d,
        t,
        &Top {
            label: "PUISSANCE 3 S",
            value: v.power_3s_w,
            unit: "W",
            reference: ("FTP", v.ftp_w),
            zones: v.power_zones,
            zone: ui.power_zone(),
            hint: "Zones : régler la FTP dans l'appli",
        },
    )?;
    four(d, t, &cells)
}

/// Page Cardio, with a heart rate sensor and no power meter: the heart rate big in the colour of its zone, then
/// its average and max over the ride, speed, and cadence with a cadence sensor, else time.
pub(super) fn ride_cardio<D: DrawTarget<Color = Rgb565>>(d: &mut D, ui: &Ui, v: &View, t: &Theme) -> Result<(), D::Error> {
    let small = t.label(&fonts::LABEL_SMALL);
    let last = if paired(v, Role::SpeedCadence) {
        (small, "CADENCE", int(v.cadence_rpm), "00", None)
    } else {
        (small, "EN MOUVEMENT", duration(v.ride.timer_s), "", None)
    };
    let cells = [
        (small, "MOY. BPM", int(v.ride.avg_heart_rate), "000", None),
        (small, "MAX. BPM", int(v.ride.max_heart_rate), "000", None),
        (small, "VITESSE KM/H", kmh(v.speed_mm_s), SPEED, None),
        last,
    ];
    zone_top(
        d,
        t,
        &Top {
            label: "FRÉQ. CARDIAQUE",
            value: v.heart_rate.map(u16::from),
            unit: "bpm",
            reference: ("FC MAX", v.hr_max),
            zones: v.hr_zones,
            zone: ui.hr_zone(),
            hint: "Zones : régler la FC max dans l'appli",
        },
    )?;
    four(d, t, &cells)
}

/// Page Altitude: altitude, then gradient, D+ and climbing speed, then the last 5 km.
pub(super) fn ride_altitude<D: DrawTarget<Color = Rgb565>>(
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
    let value = t.style(&fonts::VALUE);
    thirds(
        d,
        t,
        (BIG + 1, ROW - 1),
        &[
            grade(v, t),
            ("D+ M", int(Some(v.ride.ascent_m)), value, "000"),
            vam(v, t),
        ],
    )?;
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

/// A cell of [`thirds`]: label, value, its style and smallest slot.
type Third = (&'static str, Fmt, Style, &'static str);

/// The gradient measured, in its colour.
fn grade(v: &View, t: &Theme) -> Third {
    let value = t.style(&fonts::VALUE);
    let text = match v.grade_pct {
        Some(g) => Fmt::new(format_args!("{g} %")),
        None => Fmt::new(format_args!("{NONE}")),
    };
    ("PENTE", text, value.colored(v.grade_pct.map_or(t.fg, |g| t.grade(g))), "0 %")
}

/// The climbing speed of the last minute.
fn vam(v: &View, t: &Theme) -> Third {
    ("VAM M/H", int(v.vam_m_h), t.style(&fonts::VALUE), "000")
}

/// Three cells side by side from `y0` to `y1`, rules between them.
fn thirds<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    t: &Theme,
    (y0, y1): (i32, i32),
    cells: &[Third; 3],
) -> Result<(), D::Error> {
    vline(d, 79, y0, y1, t)?;
    vline(d, 159, y0, y1, t)?;
    let small = t.label(&fonts::LABEL_SMALL);
    for (i, (label, val, style, min)) in cells.iter().enumerate() {
        let x0 = [0, 80, 160][i];
        cell(d, rect(x0, y0, x0 + 78, y1), (&small, label), (style, val.as_str(), min), 8)?;
    }
    Ok(())
}

/// Page Climbs (a route with climbs): in a climb, what is left of it ([`climb_live`]); else the altitude, the
/// gradient and the D+ of the ride (Altitude's place), then the climbs: the last one done, the next one, the
/// following ones.
pub(super) fn ride_climbs<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    route: Option<&route::Route<'_>>,
    t: &Theme,
) -> Result<(), D::Error> {
    const ROW: i32 = 93;
    let Some(r) = route else { return Ok(()) };
    let progress = v.route.and_then(|rv| rv.progress);
    let position_known = crate::nav::has_confident_route_position(
        matches!(v.gnss, Gnss::Fix { .. }),
        progress,
    );
    // Without a current fix near the route, its along-distance is not a reliable place to measure from.
    let at = if position_known { progress.map_or(0, |p| p.along_dm) } else { 0 };
    let count = r.climbs().len();
    if position_known
        && let Some((i, c)) = r.climbs().enumerate().find(|(_, c)| (c.start_dm..c.end_dm).contains(&at))
    {
        return climb_live(d, v, r, (i, count), c, at, t);
    }
    let value = t.style(&fonts::VALUE);
    thirds(
        d,
        t,
        (TOP + 1, ROW - 1),
        &[
            ("ALTITUDE M", int(v.altitude_dm.map(|a| a / 10)), value, "000"),
            grade(v, t),
            ("D+ M", int(Some(v.ride.ascent_m)), value, "000"),
        ],
    )?;
    hline(d, ROW, t)?;
    let next = r.climbs().position(|c| c.start_dm >= at);
    let left = next.map_or(0, |n| count - n);
    let title = match left {
        0 => Fmt::new(format_args!("MONTÉES · TOUTES FAITES")),
        1 => Fmt::new(format_args!("MONTÉES · 1 RESTANTE")),
        n => Fmt::new(format_args!("MONTÉES · {n} RESTANTES")),
    };
    fonts::text(d, &t.label(&fonts::LABEL), title.as_str(), MARGIN, ROW + 13, Align::Left)?;
    // the last one done, then the next ones (all done: the last ones)
    let first = match next {
        Some(n) => n.saturating_sub(1),
        None => count.saturating_sub(4),
    };
    let mut y = ROW + 24;
    for (i, c) in r.climbs().enumerate().skip(first) {
        let is_next = Some(i) == next;
        let h = if is_next { 48 } else { 38 };
        if y + h > BOTTOM {
            break;
        }
        climb_row(d, (i, c), at, is_next, position_known, rect(0, y, W - 1, y + h - 1), t)?;
        y += h;
        if y < BOTTOM {
            hline(d, y - 1, t)?;
        }
    }
    Ok(())
}

/// A climb of the list: its gradient's colour on the left, its number, length, gradient and gain, how far it is.
fn climb_row<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    (i, c): (usize, route::Climb),
    at: u32,
    is_next: bool,
    position_known: bool,
    area: Rectangle,
    t: &Theme,
) -> Result<(), D::Error> {
    let (y0, h) = (area.top_left.y, area.size.height as i32);
    let cy = y0 + h / 2;
    let done = c.end_dm <= at;
    if is_next {
        fill(d, area, t.next)?;
    }
    let pct = (c.grade_pm() as i32 + 5) / 10;
    let bar = if is_next { 34 } else { 26 };
    let (color, _) = t.slope(pct);
    fill(d, rect(10, cy - bar / 2, 15, cy + bar / 2 - 1), if done { super::screens::mix(color, t.bg) } else { color })?;
    let bg = if is_next { t.next } else { t.bg };
    let ink = if done { t.mute } else { t.fg };
    let (name_font, detail_font): (&'static Font, &'static Font) =
        if is_next { (&fonts::HEADING_SMALL, &fonts::SUB) } else { (&fonts::BODY_BOLD, &fonts::NOTE) };
    let mut buf = [0; 4];
    let name = if done {
        Fmt::new(format_args!("{} Montée {}", char_str(icon::CHECK, &mut buf), i + 1))
    } else {
        Fmt::new(format_args!("Montée {}", i + 1))
    };
    let len = c.length_dm();
    let detail = Fmt::new(format_args!(
        "{},{} km · {},{} % · {} m",
        len / 10_000,
        len / 1_000 % 10,
        c.grade_pm() / 10,
        c.grade_pm() % 10,
        c.gain_m
    ));
    let gap = if is_next { 6 } else { 4 };
    let (nc, dc) = (name_font.cap as i32, detail_font.cap as i32);
    let top = cy - (nc + gap + dc) / 2;
    fonts::text(d, &Style::new(name_font, ink, bg), name.as_str(), 22, top + nc / 2, Align::Left)?;
    let detail_ink = if is_next { t.fg } else { t.mute };
    fonts::text(
        d,
        &Style::new(detail_font, detail_ink, bg),
        detail.as_str(),
        22,
        top + nc + gap + dc / 2,
        Align::Left,
    )?;
    if done {
        return Ok(());
    }
    if position_known {
        let away = distance(c.start_dm - at.min(c.start_dm));
        if is_next {
            let (lc, vc) = (fonts::LABEL_SMALL.cap as i32, fonts::STAT.cap as i32);
            let top = cy - (lc + 5 + vc) / 2;
            fonts::text(d, &Style::new(&fonts::LABEL_SMALL, t.mute, bg), "DANS", W - MARGIN, top + lc / 2, Align::Right)?;
            // by the advance: a changing distance keeps its right edge
            fonts::value(d, &Style::new(&fonts::STAT, t.fg, bg), away.as_str(), "", W - MARGIN, top + lc + 5 + vc / 2, Align::Right)?;
        } else {
            fonts::value(d, &Style::new(&fonts::BODY_BOLD, t.fg, bg), away.as_str(), "", W - MARGIN, cy, Align::Right)?;
        }
    }
    Ok(())
}

/// Columns of the profile of a climb.
const CLIMB_COLS: usize = 224;
/// Lengths of the stretches of a climb's profile, decimetres: the shortest that cuts it in 10 at most (more on a climb
/// over 20 km). Not under 250 m: the route's elevations are within 2 m, a gradient over 100 m would be ±4 % off.
const STRETCHES_DM: [u32; 4] = [2_500, 5_000, 10_000, 20_000];

/// In a climb (`i` of `count`), as Garmin's ClimbPro: its number and size; its profile cut in stretches of a round
/// length, each coloured by its gradient and labelled with it, the part done grey, the rider on it, the altitudes
/// of the foot and the top; then the distance, the gain and the average gradient left to the top, and the gradient
/// measured.
fn climb_live<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    v: &View,
    r: &route::Route<'_>,
    (i, count): (usize, usize),
    c: route::Climb,
    at: u32,
    t: &Theme,
) -> Result<(), D::Error> {
    const HEAD: i32 = TOP + 13;
    const LOW: i32 = 185;
    const MID: i32 = 232;
    // its number and size in the top left corner, empty above a climb's foot
    let title = fonts::text(
        d,
        &t.style(&fonts::BODY_BOLD),
        Fmt::new(format_args!("Montée {}/{count}", i + 1)).as_str(),
        MARGIN,
        HEAD,
        Align::Left,
    )?;
    let len = c.length_dm();
    let size = Fmt::new(format_args!(
        "{},{} km · {},{} % · {} m",
        len / 10_000,
        len / 1_000 % 10,
        c.grade_pm() / 10,
        c.grade_pm() % 10,
        c.gain_m
    ));
    let sized = fonts::text(d, &t.mute(&fonts::SUB), size.as_str(), MARGIN, HEAD + 19, Align::Left)?;
    // the profile, from the foot to the top
    let mut ele = [0i32; CLIMB_COLS + 1];
    r.elevations(c.start_dm, c.end_dm, &mut ele);
    let (lo, hi) = ele.iter().fold((i32::MAX, i32::MIN), |(lo, hi), &e| (lo.min(e), hi.max(e)));
    // room for the summit over the profile, for the foot and the stretches' length under it
    let (x0, top, bottom) = (8, HEAD + 10, LOW - 20);
    let span = (hi - lo).max(200) as i64;
    // under the corner's text: a climb steep from its foot is drawn lower
    let text_right = (title.top_left.x + title.size.width as i32).max(sized.top_left.x + sized.size.width as i32) + 6 - x0;
    let text_bottom = sized.top_left.y + sized.size.height as i32 + 4;
    let under = ele[..=(text_right.clamp(0, CLIMB_COLS as i32) as usize)].iter().max().map_or(0, |&e| e - lo).max(1) as i64;
    let height = ((bottom - top) as i64).min((bottom - text_bottom) as i64 * span / under);
    let y_of = |e: i32| bottom - ((e - lo) as i64 * height / span) as i32;
    let col_of = |dm: u32| ((dm - c.start_dm) as u64 * CLIMB_COLS as u64 / len.max(1) as u64) as i32;
    let me = col_of(at);
    let step = STRETCHES_DM.into_iter().find(|s| len.div_ceil(*s) <= 10).unwrap_or(STRETCHES_DM[3]);
    let n = len.div_ceil(step) as usize;
    let small = t.mute(&fonts::NOTE);
    for k in 0..n {
        let (a, b) = (c.start_dm + k as u32 * step, (c.start_dm + (k as u32 + 1) * step).min(c.end_dm));
        let mut ends = [0i32; 2];
        r.elevations(a, b, &mut ends);
        // percent, rounded
        let rise = (ends[1] - ends[0]) as i64 * 100;
        let pct = ((rise + rise.signum() * (b - a) as i64 / 2) / (b - a).max(1) as i64) as i32;
        let (color, ink) = t.slope(pct);
        let (ca, cb) = (col_of(a), col_of(b));
        for x in ca..=cb.min(CLIMB_COLS as i32) {
            let fill_c = if x < me { t.profile } else { color };
            fill(d, rect(x0 + x, y_of(ele[x as usize]), x0 + x, bottom), fill_c)?;
        }
        // a separation between stretches, and the gradient in the stretch when it is wide enough
        if k > 0 {
            fill(d, rect(x0 + ca, y_of(ele[ca as usize]), x0 + ca, bottom), t.bg)?;
        }
        // the gradient in the stretch, in the wider of its parts done and to come, where the profile holds it
        let label = Fmt::new(format_args!("{pct}"));
        let lw = fonts::NOTE_BOLD.width(label.as_str(), 0) + 4;
        let (done, ahead) = ((ca, me.clamp(ca, cb)), (me.clamp(ca, cb), cb));
        let ((pa, pb), (bg, fg)) = if done.1 - done.0 > ahead.1 - ahead.0 {
            (done, (t.profile, t.mute))
        } else {
            (ahead, (color, ink))
        };
        let (lx0, lx1) = ((pa + pb - lw) / 2, (pa + pb + lw) / 2);
        let low = (lx0.max(0)..=lx1.min(CLIMB_COLS as i32)).map(|x| y_of(ele[x as usize])).max().unwrap_or(bottom);
        if pb - pa >= lw && low <= bottom - 20 {
            fonts::text(d, &Style::new(&fonts::NOTE_BOLD, fg, bg), label.as_str(), x0 + (pa + pb) / 2, bottom - 9, Align::Center)?;
        }
    }
    // the altitudes of the top, over it, and of the foot, under it; the stretches' length
    let mut buf = [0; 4];
    let summit = Fmt::new(format_args!("{} {} m", char_str(icon::SUMMIT, &mut buf), ele[CLIMB_COLS] / 10));
    fonts::text(d, &small, summit.as_str(), x0 + CLIMB_COLS as i32, HEAD, Align::Right)?;
    let foot = Fmt::new(format_args!("{} {} m", char_str(icon::FOOT, &mut buf), ele[0] / 10));
    fonts::text(d, &small, foot.as_str(), x0, LOW - 10, Align::Left)?;
    let each = if step < 10_000 {
        Fmt::new(format_args!("bandes de {} m", step / 10))
    } else {
        Fmt::new(format_args!("bandes de {} km", step / 10_000))
    };
    fonts::text(d, &small, each.as_str(), x0 + CLIMB_COLS as i32, LOW - 10, Align::Right)?;
    disc(d, (x0 + me) as f32, y_of(ele[me as usize]) as f32, 5.0, t.me, t.bg)?;
    hline(d, LOW, t)?;
    // what is left to the top
    let mut here_top = [0; 2];
    r.elevations(at, c.end_dm, &mut here_top);
    let left_dm = c.end_dm - at;
    let gain_dm = (here_top[1] - here_top[0]).max(0);
    let avg_pm = gain_dm as u64 * 1_000 / left_dm.max(1) as u64;
    let avg_pct = ((avg_pm + 5) / 10) as i8;
    let value = t.style(&fonts::VALUE);
    // at the climbing speed of the last minute, when it climbs
    let to_top = match v.vam_m_h {
        Some(vam) if vam >= 100 => {
            let min = (gain_dm as u32 * 6 / vam as u32).max(1);
            Fmt::new(format_args!("{}:{:02}", min / 60, min % 60))
        }
        _ => Fmt::new(format_args!("{NONE}")),
    };
    let top = [
        // in metres under 1 km (docs/PRODUCT.md)
        if left_dm < 10_000 {
            ("RESTE M", Fmt::new(format_args!("{}", left_dm / 100 * 10)), value, "000")
        } else {
            ("RESTE KM", Fmt::new(format_args!("{},{}", left_dm / 10_000, left_dm / 1_000 % 10)), value, "0,0")
        },
        ("RESTE D+ M", int(Some(gain_dm / 10)), value, "000"),
        ("MOY. RESTE", Fmt::new(format_args!("{avg_pct} %")), value.colored(t.grade(avg_pct)), "0 %"),
    ];
    let bottom = [grade(v, t), vam(v, t), ("AU SOMMET", to_top, value, "0:00")];
    thirds(d, t, (LOW + 1, MID - 1), &top)?;
    hline(d, MID, t)?;
    thirds(d, t, (MID + 1, BOTTOM - 1), &bottom)
}

/// An altitude profile: the area under it; with `me` (riding), the line and the rider at its end.
pub(super) fn profile<D: DrawTarget<Color = Rgb565>>(
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

