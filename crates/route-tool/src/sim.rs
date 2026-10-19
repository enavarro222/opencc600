//! A ride replayed along a GPX through the firmware's screens, on the PC.

use std::path::Path;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use route::{Route, build, parse_gpx};
use ui::history::History;
use ui::nav::{Fix, Nav};
use ui::{Gnss, Key, Press, RideView, Ui, View};

use crate::fail;

/// Speed when the GPX has no times.
const DEFAULT_MM_S: f64 = 20.0 / 3.6 * 1000.0;

struct Fb(Vec<Rgb565>);

impl OriginDimensions for Fb {
    fn size(&self) -> Size {
        Size::new(240, 320)
    }
}

impl DrawTarget for Fb {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if (0..240).contains(&p.x) && (0..320).contains(&p.y) {
                self.0[p.y as usize * 240 + p.x as usize] = c;
            }
        }
        Ok(())
    }
}

fn screen(ui: &Ui, v: &View, r: &Route<'_>) -> Fb {
    let mut fb = Fb(vec![Rgb565::BLACK; 240 * 320]);
    ui::draw(&mut fb, ui, v, Some(r), &ui::DAY).unwrap();
    fb
}

/// Screens side by side, 8 grey pixels apart.
fn save(path: &Path, screens: &[Fb]) {
    let w = screens.len() * 248 - 8;
    let file = std::fs::File::create(path).unwrap_or_else(|e| fail(&path.display().to_string(), e));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, 320);
    enc.set_color(png::ColorType::Rgb);
    let mut out = vec![128u8; w * 320 * 3];
    for (i, s) in screens.iter().enumerate() {
        for (j, c) in s.0.iter().enumerate() {
            let (x, y) = (i * 248 + j % 240, j / 240);
            let o = (y * w + x) * 3;
            out[o..o + 3].copy_from_slice(&[c.r() << 3 | c.r() >> 2, c.g() << 2 | c.g() >> 4, c.b() << 3 | c.b() >> 2]);
        }
    }
    enc.write_header().unwrap().write_image_data(&out).unwrap();
}

pub fn run(gpx_path: &str, out: &str, every_s: u32) {
    let text = std::fs::read_to_string(gpx_path).unwrap_or_else(|e| fail(gpx_path, e));
    let gpx = parse_gpx(&text).unwrap_or_else(|e| fail(gpx_path, format!("{e:?}")));
    let file = build(&gpx, 3.0);
    let r = Route::parse(&file).unwrap();
    std::fs::create_dir_all(out).unwrap_or_else(|e| fail(out, e));

    // the GPX's points on the route's plane, with their times (or 20 km/h)
    let e7 = |d: f64| (d * 1e7).round() as i32;
    let pts: Vec<(i32, i32, f64)> = gpx
        .points
        .iter()
        .map(|&(lat, lon, ele)| {
            let (x, y) = r.project(e7(lat), e7(lon));
            (x, y, ele.unwrap_or(0.0))
        })
        .collect();
    let mut times = vec![0.0f64];
    for i in 1..pts.len() {
        let d = (((pts[i].0 - pts[i - 1].0) as f64).hypot((pts[i].1 - pts[i - 1].1) as f64)) * 100.0;
        times.push(times[i - 1] + d / DEFAULT_MM_S);
    }
    if gpx.times.iter().all(Option::is_some) {
        let t0 = gpx.times[0].unwrap();
        times = gpx.times.iter().map(|t| t.unwrap() - t0).collect();
    }
    let start_unix = gpx.times.first().copied().flatten().unwrap_or(1_792_316_426.0) as i64;
    let tz = settings::Settings::default().tz;

    let mut ui = Ui::new();
    ui.set_context(true, false);
    let mut nav = Nav::default();
    let mut history = History::new();
    let mut ride = RideView::default();
    let (mut climb_ref, mut ascent, mut descent) = (None::<f64>, 0.0, 0.0);
    let mut last: Option<(i32, i32)> = None;
    let mut max_off = (0u32, 0u32);
    let (mut seg, mut frames) = (0, 0);
    let end = *times.last().unwrap() as u32;
    let mut view = View::default();
    for t in 0..=end {
        // the position at time t, between two GPX points
        while seg + 2 < pts.len() && times[seg + 1] < t as f64 {
            seg += 1;
        }
        let (a, b) = (pts[seg], pts[seg + 1]);
        let f = ((t as f64 - times[seg]) / (times[seg + 1] - times[seg]).max(1e-9)).clamp(0.0, 1.0);
        let (x, y, ele) = (
            a.0 as f64 + (b.0 - a.0) as f64 * f,
            a.1 as f64 + (b.1 - a.1) as f64 * f,
            a.2 + (b.2 - a.2) * f,
        );
        let (xi, yi) = (x.round() as i32, y.round() as i32);
        // speed and course from the last second, distance like the firmware's odometer (moving only)
        let (speed, course) = match last {
            Some((lx, ly)) => {
                let (dx, dy) = ((xi - lx) as f64, (yi - ly) as f64);
                let speed = (dx.hypot(dy) * 100.0) as u32;
                let course = (dx.atan2(dy).to_degrees().rem_euclid(360.0) * 100.0) as u16;
                (speed, Some(course))
            }
            None => (0, None),
        };
        last = Some((xi, yi));
        if speed >= 500 {
            ride.distance_cm += speed / 10;
        }
        ride.timer_s = t;
        ride.elapsed_s = t;
        ride.max_speed_mm_s = ride.max_speed_mm_s.max(speed);
        let reference = *climb_ref.get_or_insert(ele);
        if ele >= reference + 2.0 {
            ascent += ele - reference;
            climb_ref = Some(ele);
        } else if ele <= reference - 2.0 {
            descent += reference - ele;
            climb_ref = Some(ele);
        }
        ride.ascent_m = ascent as u16;
        ride.descent_m = descent as u16;
        let ele_dm = (ele * 10.0).round() as i32;
        history.push(ride.distance_cm, ele_dm);

        // the fix back to latitude and longitude through the route's projection: the firmware's input
        let fix = unproject(&r, xi, yi).map(|(lat_e7, lon_e7)| Fix {
            lat_e7,
            lon_e7,
            speed_mm_s: Some(speed),
            course_cdeg: course,
        });
        let unix = start_unix + t as i64;
        let local = unix + tz.offset_s(unix) as i64;
        view = View {
            clock: Some(((local.rem_euclid(86_400) / 3600) as u8, (local.rem_euclid(3600) / 60) as u8)),
            battery_pct: Some(100u32.saturating_sub(t / 600) as u8),
            gnss: Gnss::Fix { used: 9 },
            speed_mm_s: Some(if speed < 500 { 0 } else { speed }),
            altitude_dm: Some(ele_dm),
            grade_pct: history.grade_pct(ride.distance_cm, ele_dm),
            ride,
            recent: history.recent(),
            whole: history.whole(),
            rides_stored: Some(0),
            route: Some(nav.update(&r, fix)),
            ..Default::default()
        };
        if t == 0 {
            ui.key(Key::BottomRight, Press::Click, 0, &view);
        }
        ui.tick(t as u64 * 1000, Some(true), &view);
        let p = view.route.and_then(|rv| rv.progress).unwrap();
        if p.off_dm > max_off.0 {
            max_off = (p.off_dm, t);
        }
        if t % every_s == 0 || t == end {
            let map = screen(&ui, &view, &r);
            let mut other = ui;
            other.key(Key::BottomLeft, Press::Click, 0, &view);
            let essential = screen(&other, &view, &r);
            save(&Path::new(out).join(format!("t{t:05}.png")), &[map, essential]);
            frames += 1;
            println!(
                "{:>2}:{:02}:{:02}  {:5.1} km along, {:5.1} m off, {:4.1} km left",
                t / 3600,
                t / 60 % 60,
                t % 60,
                p.along_dm as f64 / 10_000.0,
                p.off_dm as f64 / 10.0,
                r.info.length_dm.saturating_sub(p.along_dm) as f64 / 10_000.0
            );
        }
    }
    let _ = view;
    println!(
        "{frames} frames in {out}; farthest from the route: {:.1} m at {}:{:02}:{:02}",
        max_off.0 as f64 / 10.0,
        max_off.1 / 3600,
        max_off.1 / 60 % 60,
        max_off.1 % 60
    );
}

/// The inverse of the route's projection, near enough for the simulator (the device gets latitude and longitude).
fn unproject(r: &Route<'_>, x: i32, y: i32) -> Option<(i32, i32)> {
    // the projection is linear: two probes give its scale
    let (x1, _) = r.project(r.info.lat0_e7, r.info.lon0_e7 + 10_000_000);
    let (_, y1) = r.project(r.info.lat0_e7 + 10_000_000, r.info.lon0_e7);
    if x1 == 0 || y1 == 0 {
        return None;
    }
    let lon = r.info.lon0_e7 as i64 + x as i64 * 10_000_000 / x1 as i64;
    let lat = r.info.lat0_e7 as i64 + y as i64 * 10_000_000 / y1 as i64;
    Some((lat as i32, lon as i32))
}
