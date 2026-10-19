//! Draws every screen with sample values: no glyph missing from the fonts (debug builds stop on one), no key label
//! wider than its box, no text out of the screen; PNGs to look at in `target/tmp/screens/` (compare with the
//! prototype's, tools/prototype_shots.py and tools/compare_screens.py).

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use ui::fonts::Line;
use ui::{Gnss, Key, Press, Profile, RideView, Theme, Ui, View};

struct Fb(Vec<Rgb565>);

impl OriginDimensions for Fb {
    fn size(&self) -> Size {
        Size::new(240, 320)
    }
}

impl DrawTarget for Fb {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if (0..240).contains(&p.x) && (0..320).contains(&p.y) {
                self.0[p.y as usize * 240 + p.x as usize] = c;
            }
        }
        Ok(())
    }
}

fn profile(n: usize, f: impl Fn(usize) -> i16) -> Profile {
    let mut p = Profile::default();
    for i in 0..n {
        p.points[i] = f(i);
    }
    p.len = n as u8;
    p
}

/// The prototype's sample ride.
fn view() -> View {
    View {
        clock: Some((10, 29)),
        battery_pct: Some(81),
        gnss: Gnss::Fix { used: 9 },
        speed_mm_s: Some(9_000),
        altitude_dm: Some(6_120),
        grade_pct: Some(4),
        heart_rate: None,
        ride: RideView {
            timer_s: 6128,
            elapsed_s: 6128 + 12 * 60,
            distance_cm: 4_863_000,
            ascent_m: 812,
            descent_m: 640,
            max_speed_mm_s: 17_400,
        },
        recent: profile(50, |i| 5_700 + (i as i16 * 9) + ((i as i16 * 7) % 23) * 4),
        whole: profile(50, |i| {
            4_000 + ((i as f32 / 5.0).sin() * 900.0) as i16 + i as i16 * 20
        }),
        rides_stored: Some(2),
        resumable: true,
        route: None,
    }
}

fn save(name: &str, ui: &Ui, v: &View, theme: &Theme) {
    let mut fb = Fb(vec![Rgb565::BLACK; 240 * 320]);
    let file = test_route();
    let r = route::Route::parse(&file).unwrap();
    ui::draw(&mut fb, ui, v, v.route.map(|_| &r), theme).unwrap();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("screens");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 240, 320);
    enc.set_color(png::ColorType::Rgb);
    let mut w = enc.write_header().unwrap();
    let data: Vec<u8> =
        fb.0.iter()
            .flat_map(|c| {
                [
                    c.r() << 3 | c.r() >> 2,
                    c.g() << 2 | c.g() >> 4,
                    c.b() << 3 | c.b() >> 2,
                ]
            })
            .collect();
    w.write_image_data(&data).unwrap();
}

/// A 6 km loop: 1.5 km north, a bend east, back south (synthetic, near the prototype's place).
fn test_route() -> Vec<u8> {
    let mut gpx = String::from("<gpx><trk><name>Boucle Montagne Noire et Lacs</name><trkseg>");
    let (lat0, lon0) = (43.45_f64, 2.05_f64);
    let km_lat = 1.0 / 111.319;
    let km_lon = km_lat / lat0.to_radians().cos();
    for i in 0..=300 {
        let a = i as f64 / 300.0 * std::f64::consts::TAU;
        let (n, e) = (0.95 * (1.0 - a.cos()), 0.95 * a.sin() * (1.0 + 0.3 * (2.0 * a).sin()));
        gpx += &format!(
            "<trkpt lat=\"{:.6}\" lon=\"{:.6}\"><ele>{:.1}</ele></trkpt>",
            lat0 + n * km_lat,
            lon0 + e * km_lon,
            300.0 + 80.0 * (a / 2.0).sin()
        );
    }
    gpx += "</trkseg></trk></gpx>";
    route::build(&route::parse_gpx(&gpx).unwrap(), 3.0)
}

/// The rider a quarter round the loop, heading north east.
fn with_route(v: View, ride: bool) -> View {
    let file = test_route();
    let r = route::Route::parse(&file).unwrap();
    let pts: Vec<_> = r.points().collect();
    let me = pts[pts.len() / 5];
    let progress = r.locate(me.x, me.y, None);
    View {
        route: Some(ui::RouteView {
            name: Line::new(format_args!("{}", r.name())),
            length_dm: r.info.length_dm,
            ascent_m: r.info.ascent_m,
            me: ride.then_some((me.x + 40, me.y)),
            heading_cdeg: Some(3_000),
            progress: ride.then_some(progress),
        }),
        ..v
    }
}

/// The states of tools/prototype_shots.py, under the same names.
#[test]
fn every_screen() {
    use Key::*;
    for (theme, suffix) in [(ui::DAY, ""), (ui::NIGHT, "-night")] {
        let v = view();
        let home = View {
            clock: Some((8, 47)),
            ..v
        };
        let mut ui = Ui::new();
        let shot = |ui: &Ui, v: &View, name: &str| save(&format!("{name}{suffix}"), ui, v, &theme);
        let key = |ui: &mut Ui, k: Key| {
            ui.key(k, Press::Click, 0, &v);
        };
        let searching = View {
            gnss: Gnss::Searching {
                in_view: 5,
                progress: 10,
            },
            ..home
        };
        shot(&ui, &searching, "home-search");
        shot(&ui, &home, "home");
        key(&mut ui, BottomLeft);
        shot(&ui, &home, "menu");
        key(&mut ui, TopLeft);
        key(&mut ui, BottomRight);
        ui.tick(0, Some(true), &v);
        shot(&ui, &v, "ride-page1");
        key(&mut ui, BottomLeft);
        shot(&ui, &v, "ride-page2");
        key(&mut ui, BottomLeft);
        let lost = View {
            gnss: Gnss::Searching {
                in_view: 3,
                progress: 0,
            },
            ..v
        };
        ui.tick(1, Some(true), &lost);
        shot(&ui, &lost, "ride-gps-lost");
        ui.tick(2, Some(true), &v);
        shot(&ui, &v, "ride-gps-found");
        ui.tick(2 + ui::INFO_MS, Some(true), &v);
        key(&mut ui, BottomRight);
        shot(&ui, &home, "paused");
        key(&mut ui, TopLeft);
        shot(&ui, &home, "pause-menu");
        key(&mut ui, TopLeft);
        key(&mut ui, BottomLeft);
        shot(&ui, &home, "confirm-stop");
        key(&mut ui, BottomRight);
        shot(&ui, &home, "saved");
        let mut resume = Ui::new();
        resume.found_paused_ride();
        shot(&resume, &home, "resume");
        // scrolling pages, the indicator under the page dots
        let mut scroll = Ui::new();
        scroll.key(BottomRight, Press::Click, 0, &v);
        scroll.key(TopLeft, Press::Click, 0, &v);
        shot(&scroll, &v, "ride-scrolling");
        let mut info = Ui::new();
        info.inform(
            "Pas encore l'heure",
            Line::new(format_args!("Attends le GPS")),
            0,
        );
        shot(&info, &home, "home-info");
    }
    // with a route: home, then the map at each zoom, the whole route before the fix, off the route
    let mut ui = Ui::new();
    ui.set_context(true, false);
    let home = with_route(view(), false);
    save("route-home", &ui, &home, &ui::DAY);
    ui.key(Key::BottomRight, Press::Click, 0, &home);
    let riding = with_route(view(), true);
    ui.tick(0, Some(true), &riding);
    for z in ["route-map-1km", "route-map-2km", "route-map-500m"] {
        save(z, &ui, &riding, &ui::DAY);
        ui.key(Key::TopLeft, Press::Click, 0, &riding);
    }
    save("route-map-night", &ui, &riding, &ui::NIGHT);
    let mut lost = riding;
    if let Some(r) = lost.route.as_mut() {
        r.me = None;
    }
    save("route-map-no-fix", &ui, &lost, &ui::DAY);
    let mut off = riding;
    if let Some(r) = off.route.as_mut() {
        r.progress = r.progress.map(|p| route::Progress { off_dm: 1_230, ..p });
    }
    ui.tick(1, Some(true), &off);
    save("route-off", &ui, &off, &ui::DAY);
    ui.key(Key::BottomLeft, Press::Click, 0, &riding);
    save("route-essential", &ui, &riding, &ui::DAY);
    // unknown values, a slow speed (its comma where 32,4 has it), a low battery
    let empty = View::default();
    let mut ui = Ui::new();
    save("empty-home", &ui, &empty, &ui::DAY);
    ui.key(Key::BottomRight, Press::Click, 0, &empty);
    save("empty-ride", &ui, &empty, &ui::DAY);
    let slow = View {
        speed_mm_s: Some(2_722),
        battery_pct: Some(9),
        heart_rate: Some(142),
        ..view()
    };
    save("ride-slow-hr", &ui, &slow, &ui::DAY);
}
