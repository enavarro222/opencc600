//! Draws every screen with sample values: no glyph missing from the fonts (debug builds stop on one), no key label
//! wider than its box, no text out of the screen; PNGs to look at in `target/tmp/screens/` (compare with the
//! prototype's, tools/prototype_shots.py and tools/compare_screens.py).

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use ui::fonts::Line;
use ui::{Gnss, Key, Press, Profile, RideView, Theme, Ui, View};

/// A frame buffer, the panel's size or any other (the top bars' sheet).
struct Fb {
    w: u32,
    h: u32,
    px: Vec<Rgb565>,
}

impl Fb {
    fn new(w: u32, h: u32, c: Rgb565) -> Self {
        Fb { w, h, px: vec![c; (w * h) as usize] }
    }

    fn png(&self, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("screens");
        let path = dir.join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.w, self.h);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        let data: Vec<u8> = self
            .px
            .iter()
            .flat_map(|c| [c.r() << 3 | c.r() >> 2, c.g() << 2 | c.g() >> 4, c.b() << 3 | c.b() >> 2])
            .collect();
        w.write_image_data(&data).unwrap();
    }
}

impl OriginDimensions for Fb {
    fn size(&self) -> Size {
        Size::new(self.w, self.h)
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
            if (0..self.w as i32).contains(&p.x) && (0..self.h as i32).contains(&p.y) {
                self.px[(p.y as u32 * self.w + p.x as u32) as usize] = c;
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
        plugged: false,
        phone: false,
        gnss: Gnss::Fix { used: 9 },
        speed_mm_s: Some(9_000),
        altitude_dm: Some(6_120),
        grade_pct: Some(4),
        vam_m_h: Some(640),
        heart_rate: None,
        cadence_rpm: None,
        power_w: None,
        power_3s_w: None,
        wheel_speed_mm_s: None,
        ftp_w: 0,
        hr_max: 0,
        power_zones: ui::Zones::parse(settings::POWER_ZONES).unwrap(),
        hr_zones: ui::Zones::parse(settings::HR_ZONES).unwrap(),
        sensors: Default::default(),
        ride: RideView {
            timer_s: 6128,
            elapsed_s: 6128 + 12 * 60,
            distance_cm: 4_863_000,
            ascent_m: 812,
            descent_m: 640,
            max_speed_mm_s: 17_400,
            avg_power_w: None,
            avg_heart_rate: None,
            max_heart_rate: None,
        },
        recent: profile(50, |i| 5_700 + (i as i16 * 9) + ((i as i16 * 7) % 23) * 4),
        whole: profile(50, |i| {
            4_000 + ((i as f32 / 5.0).sin() * 900.0) as i16 + i as i16 * 20
        }),
        rides_stored: Some(2),
        record_hours: Some(58),
        saving: false,
        resumable: true,
        route: None,
        route_set_aside: false,
        stream_lost: false,
        write_failure: None,
        sparse: false,
        rides: Default::default(),
    }
}

fn save(name: &str, ui: &Ui, v: &View, theme: &Theme) {
    save_with(name, ui, v, &test_route(), theme);
}

/// [`save`] with the route of `file`.
fn save_with(name: &str, ui: &Ui, v: &View, file: &[u8], theme: &Theme) {
    screen(ui, v, file, theme).png(name);
}

/// The whole panel.
fn screen(ui: &Ui, v: &View, file: &[u8], theme: &Theme) -> Fb {
    let mut fb = Fb::new(240, 320, Rgb565::BLACK);
    let r = route::Route::parse(file).unwrap();
    ui::draw(&mut fb, ui, v, v.route.map(|_| &r), None, theme).unwrap();
    fb
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
        shot(&ui, &View { phone: true, ..home }, "home-phone");
        shot(&ui, &View { rides_stored: Some(14), record_hours: Some(3), ..home }, "home-storage-low");
        // a ride refused for want of room
        let mut refused = Ui::new();
        refused.inform("Mémoire pleine", ui::fonts::Line::new(format_args!("Envoie les sorties à l'appli")), 0);
        shot(&refused, &View { rides_stored: Some(60), record_hours: Some(0), ..home }, "home-storage-full");
        // the rides screen, from a refusal and from the menu, and its confirmation
        let card = ui::RideCard {
            weekday: 6,
            day: 12,
            month: 9,
            hour: 8,
            minute: 47,
            timer_s: 3 * 3600 + 10 * 60,
            distance_cm: 7_420_000,
            sent: false,
        };
        let full = View {
            record_hours: Some(0),
            rides: ui::RidesView { count: 14, card: Some(ui::RideCard { sent: true, ..card }), can_start: false },
            ..home
        };
        let mut rides = Ui::new();
        rides.make_room();
        shot(&rides, &full, "rides-making-room");
        let room = View {
            record_hours: Some(3),
            rides: ui::RidesView { can_start: true, count: 13, ..full.rides },
            ..full
        };
        shot(&rides, &room, "rides-room-made");
        let mut rides = Ui::new();
        let listed = View { record_hours: Some(41), rides: ui::RidesView { card: Some(card), ..full.rides }, ..home };
        rides.key(Key::BottomLeft, Press::Click, 0, &listed);
        let mut menu = rides;
        shot(&menu, &View { resumable: true, ..with_route(listed, false) }, "menu-five");
        // without a route: resume, sensors, rides, status
        menu.key(Key::BottomLeft, Press::Click, 0, &listed);
        menu.key(Key::BottomLeft, Press::Click, 0, &listed);
        menu.key(Key::BottomRight, Press::Click, 0, &listed);
        assert_eq!(menu.screen(), ui::Screen::Rides);
        shot(&menu, &listed, "rides");
        menu.key(Key::BottomRight, Press::Click, 0, &listed);
        shot(&menu, &listed, "rides-confirm-delete");
        key(&mut ui, BottomLeft);
        shot(&ui, &home, "menu");
        // the last entry
        let mut about = ui;
        while let (items, at) = about.menu(&v)
            && items[at as usize] != Some(ui::Item::About)
        {
            key(&mut about, BottomLeft);
        }
        key(&mut about, BottomRight);
        assert_eq!(about.screen(), ui::Screen::About);
        shot(&about, &home, "about");
        key(&mut about, TopLeft);
        assert_eq!(about.screen(), ui::Screen::Menu);
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
        let full = View { write_failure: Some(ui::WriteFailure::Full), ..v };
        ui.tick(3 + ui::INFO_MS, Some(true), &full);
        shot(&ui, &full, "ride-storage-full");
        ui.tick(4 + ui::INFO_MS, Some(true), &v);
        shot(&ui, &v, "ride-recording-again");
        ui.tick(4 + 2 * ui::INFO_MS, Some(true), &v);
        shot(&ui, &View { sparse: true, record_hours: Some(6), ..v }, "ride-sparse");
        shot(&ui, &View { battery_pct: Some(18), ..v }, "ride-battery-low");
        shot(&ui, &View { battery_pct: Some(7), ..v }, "ride-battery-very-low");
        shot(&ui, &View { battery_pct: Some(100), ..v }, "ride-battery-full");
        shot(&ui, &View { phone: true, ..v }, "ride-phone");
        shot(&ui, &View { battery_pct: Some(64), plugged: true, ..v }, "ride-plugged");
        shot(&ui, &View { battery_pct: Some(100), plugged: true, ..v }, "ride-plugged-full");
        key(&mut ui, BottomRight);
        shot(&ui, &home, "paused");
        shot(&ui, &View { sparse: true, record_hours: Some(6), ..home }, "paused-sparse");
        key(&mut ui, TopLeft);
        shot(&ui, &home, "pause-menu");
        // each entry names its own action on the select key: every label must fit
        let mut each = ui;
        let routed = with_route(home, true);
        for i in 0..5 {
            shot(&each, &routed, &format!("pause-menu-{i}"));
            each.key(BottomLeft, Press::Click, 0, &routed);
        }
        key(&mut ui, TopLeft);
        key(&mut ui, BottomLeft);
        shot(&ui, &home, "confirm-stop");
        key(&mut ui, BottomRight);
        shot(&ui, &home, "saved");
        shot(&ui, &View { saving: true, ..home }, "saving");
        let mut resume = Ui::new();
        resume.found_paused_ride();
        shot(&resume, &home, "resume");
        // erasing: an empty ride from the finish key, a real one from the pause menu with two confirmations
        let empty = View { ride: ui::RideView { timer_s: 72, distance_cm: 0, ..home.ride }, ..home };
        let mut erase = resume;
        erase.key(BottomLeft, Press::Click, 0, &empty);
        shot(&erase, &empty, "confirm-empty");
        let mut erase = resume;
        erase.key(TopLeft, Press::Click, 0, &home);
        for _ in 0..3 {
            erase.key(BottomLeft, Press::Click, 0, &home);
        }
        shot(&erase, &home, "pause-menu-discard");
        erase.key(BottomRight, Press::Click, 0, &home);
        shot(&erase, &home, "confirm-discard");
        erase.key(BottomRight, Press::Click, 0, &home);
        shot(&erase, &home, "confirm-discard-again");
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
        // a file coming from the app: the arrow and the bar of what is done
        let mut receiving = Ui::new();
        receiving.progress("Réception de la carte", Line::new(format_args!("37 %")), 37, 100, 0);
        shot(&receiving, &home, "home-receiving");
    }
    // with a route: home, then the map at each zoom, the whole route before the fix, off the route
    let mut ui = Ui::new();
    ui.set_context(true, false, false, ui::Effort::None);
    let home = with_route(view(), false);
    save("route-home", &ui, &home, &ui::DAY);
    ui.key(Key::BottomRight, Press::Click, 0, &home);
    let riding = with_route(view(), true);
    ui.tick(0, Some(true), &riding);
    for z in ["route-map-1km", "route-map-2km", "route-map-whole", "route-map-250m", "route-map-500m"] {
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
    ui.tick(1 + ui::OFF_ROUTE_HINT_MS, Some(true), &off);
    save("route-off-hint", &ui, &off, &ui::DAY);
    ui.tick(1 + ui::OFF_ROUTE_ALERT_MS, Some(true), &off);
    save("route-off-after", &ui, &off, &ui::DAY);
    let mut far = off;
    if let Some(r) = far.route.as_mut() {
        r.progress = r.progress.map(|p| route::Progress { off_dm: 99_990, ..p });
    }
    ui.tick(2 + ui::OFF_ROUTE_ALERT_MS, Some(true), &far);
    ui.tick(2 + 2 * ui::OFF_ROUTE_ALERT_MS, Some(true), &far);
    save("route-off-far", &ui, &far, &ui::DAY);
    save("route-off-far-phone", &ui, &View { phone: true, ..far }, &ui::DAY);
    save("route-off-far-plugged", &ui, &View { plugged: true, ..far }, &ui::DAY);
    ui.key(Key::BottomLeft, Press::Click, 0, &riding);
    save("route-essential", &ui, &riding, &ui::DAY);
    // the automatic power off announced, a ride still for a while
    let mut ui = Ui::new();
    ui.set_auto_off_min(10);
    let still = View { speed_mm_s: Some(0), ..view() };
    ui.key(Key::BottomRight, Press::Click, 0, &still);
    ui.tick(0, Some(true), &still);
    ui.tick(9 * 60_000 + 15_000, Some(true), &still);
    save("auto-off", &ui, &still, &ui::DAY);
    // the logo at power-on
    let mut ui = Ui::new();
    ui.show_splash();
    save("splash", &ui, &view(), &ui::DAY);
    save("splash-night", &ui, &view(), &ui::NIGHT);
    ui.set_identity("CC700", "OpenCC700 AEA8");
    save("splash-cc700", &ui, &view(), &ui::DAY);
    ui.end_splash();
    assert_eq!(ui.screen(), ui::Screen::Home);
    // started by the charger: the charging screen, its animation, full, unplugged; night
    for (name, pct, plugged, at) in [
        ("charging", 64, true, 0),
        ("charging-step", 64, true, ui::CHARGE_CYCLE_MS / 2),
        ("charging-low", 9, true, 0),
        ("charging-full", 100, true, 0),
        ("charging-unplugged", 64, false, 0),
    ] {
        let mut ui = Ui::new();
        ui.show_charging();
        let v = View { battery_pct: Some(pct), plugged, stream_lost: true, ..view() };
        ui.tick(at, None, &v);
        save(name, &ui, &v, &ui::DAY);
        if name == "charging" {
            save("charging-night", &ui, &v, &ui::NIGHT);
            assert_eq!(ui.key(Key::TopLeft, Press::Long, at, &v), Some(ui::Command::WakeUp));
            assert_eq!(ui.screen(), ui::Screen::Splash);
            ui.end_splash();
            assert_eq!(ui.screen(), ui::Screen::Home);
        }
    }
    // a free ride: the route set aside from the menu, kept
    let mut ui = Ui::new();
    let home = with_route(view(), false);
    ui.key(Key::BottomLeft, Press::Click, 0, &home);
    save("menu-route", &ui, &home, &ui::DAY);
    ui.key(Key::BottomLeft, Press::Click, 0, &home);
    assert_eq!(ui.key(Key::BottomRight, Press::Click, 0, &home), Some(ui::Command::FollowRoute(false)));
    let free = View { route: None, route_set_aside: true, ..home };
    save("home-free-ride", &ui, &free, &ui::DAY);
    ui.key(Key::BottomLeft, Press::Click, 0, &free);
    ui.key(Key::BottomLeft, Press::Click, 0, &free);
    save("menu-follow-route", &ui, &free, &ui::DAY);
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
    let charging = View {
        battery_pct: Some(64),
        plugged: true,
        ..view()
    };
    save("ride-charging", &ui, &charging, &ui::DAY);
    save("ride-charging-night", &ui, &charging, &ui::NIGHT);
}

/// The sensors screens: the roles with a sensor connected, one searching, one unpaired; then a scan.
#[test]
fn effort_pages() {
    use ui::{Effort, Link, Page, SensorRow, Sensors};
    let row = |n: &str| SensorRow { link: Link::Connected, name: Some(Line::new(format_args!("{n}"))) };
    let power_and_hr = View {
        heart_rate: Some(142),
        cadence_rpm: Some(88),
        power_w: Some(231),
        power_3s_w: Some(215),
        ftp_w: 230,
        hr_max: 185,
        sensors: Sensors { roles: [row("H9Z 12381"), SensorRow::default(), row("Assioma")], ..Default::default() },
        ride: RideView { avg_power_w: Some(198), avg_heart_rate: Some(136), max_heart_rate: Some(171), ..view().ride },
        ..view()
    };
    let power_only = View {
        heart_rate: None,
        sensors: Sensors { roles: [SensorRow::default(), SensorRow::default(), row("Assioma")], ..Default::default() },
        ..power_and_hr
    };
    // the power meter lost: its cells stay, with "–"
    let lost = View { power_w: None, power_3s_w: None, cadence_rpm: None, ..power_and_hr };
    // no FTP nor max heart rate: no zones
    let unset = View { ftp_w: 0, hr_max: 0, ..power_and_hr };
    // other zones: 6 of Zwift, the last one
    let zwift = View { power_3s_w: Some(420), power_zones: ui::Zones::parse("59,75,89,104,118").unwrap(), ..power_and_hr };
    let hr_only = View {
        power_w: None,
        power_3s_w: None,
        cadence_rpm: None,
        sensors: Sensors { roles: [row("H9Z 12381"), SensorRow::default(), SensorRow::default()], ..Default::default() },
        ..power_and_hr
    };
    let shots = [
        (&power_and_hr, Effort::Power, Page::Effort, "effort-power"),
        (&power_only, Effort::Power, Page::Effort, "effort-power-only"),
        (&lost, Effort::Power, Page::Effort, "effort-power-lost"),
        (&unset, Effort::Power, Page::Effort, "effort-power-no-ftp"),
        (&zwift, Effort::Power, Page::Effort, "effort-power-z6-of-6"),
        (&hr_only, Effort::Heart, Page::Cardio, "effort-cardio"),
        (&View { hr_max: 0, ..hr_only }, Effort::Heart, Page::Cardio, "effort-cardio-no-max"),
    ];
    for (theme, suffix) in [(ui::DAY, ""), (ui::NIGHT, "-night")] {
        for (v, effort, page, name) in shots {
            let mut ui = Ui::new();
            ui.set_context(false, false, true, effort);
            ui.key(Key::BottomRight, Press::Click, 0, v);
            ui.tick(0, Some(true), v);
            assert_eq!(ui.page(), Some(page));
            save(&format!("{name}{suffix}"), &ui, v, &theme);
        }
        // Essential with the heart rate and its zone
        let mut ui = Ui::new();
        ui.set_context(false, false, false, Effort::Heart);
        ui.key(Key::BottomRight, Press::Click, 0, &hr_only);
        ui.tick(0, Some(true), &hr_only);
        assert_eq!(ui.page(), Some(Page::Essential));
        save(&format!("ride-page1-hr{suffix}"), &ui, &hr_only, &theme);
    }
}

#[test]
fn sensor_screens() {
    use Key::*;
    use ui::{Found, Link, SensorRow, Sensors};
    let name = |n: &str| Some(Line::new(format_args!("{n}")));
    let found = |n: &str, rssi_dbm: i8, paired: bool| {
        Some(Found {
            name: Line::new(format_args!("{n}")),
            rssi_dbm,
            paired,
        })
    };
    let v = View {
        heart_rate: Some(142),
        cadence_rpm: Some(91),
        wheel_speed_mm_s: Some(8_330),
        sensors: Sensors {
            roles: [
                SensorRow { link: Link::Connected, name: name("HRM-Pro") },
                SensorRow { link: Link::Searching, name: name("Wahoo CADENCE") },
                SensorRow::default(),
            ],
            found: [found("HRM-Pro", -48, true), found("Polar H10", -71, false), None, None],
            ..Default::default()
        },
        ..view()
    };
    for (theme, suffix) in [(ui::DAY, ""), (ui::NIGHT, "-night")] {
        let mut ui = Ui::new();
        let shot = |ui: &Ui, v: &View, n: &str| save(&format!("{n}{suffix}"), ui, v, &theme);
        ui.key(BottomLeft, Press::Click, 0, &v); // menu
        ui.key(BottomLeft, Press::Click, 0, &v); // to the sensors
        shot(&ui, &v, "menu-sensors");
        ui.key(BottomRight, Press::Click, 0, &v);
        shot(&ui, &v, "sensors");
        assert_eq!(ui.key(BottomRight, Press::Click, 0, &v), Some(ui::Command::Scan(ui::Role::HeartRate)));
        ui.key(BottomLeft, Press::Click, 0, &v);
        shot(&ui, &v, "sensor-scan");
        // nothing found yet, for an unpaired role
        let empty = View { sensors: Sensors { found: [None; 4], ..v.sensors }, ..v };
        ui.key(TopLeft, Press::Click, 0, &v);
        ui.key(BottomLeft, Press::Click, 0, &v);
        ui.key(BottomLeft, Press::Click, 0, &v);
        ui.key(BottomRight, Press::Click, 0, &empty);
        shot(&ui, &empty, "sensor-scan-empty");
    }
    // a power meter paired: its calibration after the roles
    let mut power = v;
    power.sensors.roles[2] = SensorRow { link: Link::Connected, name: name("Assioma") };
    power.power_w = Some(0);
    for (theme, suffix) in [(ui::DAY, ""), (ui::NIGHT, "-night")] {
        let mut ui = Ui::new();
        let shot = |ui: &Ui, v: &View, n: &str| save(&format!("{n}{suffix}"), ui, v, &theme);
        ui.key(BottomLeft, Press::Click, 0, &power); // menu
        ui.key(BottomLeft, Press::Click, 0, &power); // to the sensors
        ui.key(BottomRight, Press::Click, 0, &power);
        for _ in 0..3 {
            ui.key(BottomLeft, Press::Click, 0, &power);
        }
        assert!(ui.calibrate_row());
        shot(&ui, &power, "sensors-calibrate");
        ui.key(BottomRight, Press::Click, 0, &power);
        assert_eq!(ui.screen(), ui::Screen::Calibrate);
        shot(&ui, &power, "calibrate");
        for (c, n) in [
            (ui::Calibration::Running, "calibrate-running"),
            (ui::Calibration::Done(-312), "calibrate-done"),
            (ui::Calibration::NotConnected, "calibrate-failed"),
        ] {
            let v = View { sensors: Sensors { calibration: c, ..power.sensors }, ..power };
            shot(&ui, &v, n);
        }
    }
}

/// 16 km winding north, three climbs: 2 km at 5 % from km 1, 6 km at 7 % (steeper in the middle) from km 5, 1.5 km
/// at 4 % from km 13.
fn climbs_route() -> Vec<u8> {
    let mut gpx = String::from("<gpx><trk><name>Trois montées</name><trkseg>");
    let (lat0, lon0) = (43.45_f64, 2.05_f64);
    let km_lat = 1.0 / 111.319;
    let km_lon = km_lat / lat0.to_radians().cos();
    let mut ele = 300.0;
    for i in 0..=1600 {
        let km = i as f64 / 100.0;
        let grade = match km {
            k if (1.0..3.0).contains(&k) => 0.05,
            k if (3.0..4.0).contains(&k) => -0.04,
            k if (5.0..7.0).contains(&k) => 0.06,
            k if (7.0..9.0).contains(&k) => 0.09,
            k if (9.0..11.0).contains(&k) => 0.06,
            k if (11.0..12.5).contains(&k) => -0.08,
            k if (13.0..14.5).contains(&k) => 0.04,
            _ => 0.0,
        };
        if i > 0 {
            ele += grade * 10.0;
        }
        gpx += &format!(
            "<trkpt lat=\"{:.6}\" lon=\"{:.6}\"><ele>{:.1}</ele></trkpt>",
            lat0 + km * km_lat,
            lon0 + 0.3 * (km * 1.3).sin() * km_lon,
            ele
        );
    }
    gpx += "</trkseg></trk></gpx>";
    route::build(&route::parse_gpx(&gpx).unwrap(), 3.0)
}

/// The climbs page: before the first climb, on the way to the second one, in it, all done; a route without climbs
/// keeps Altitude.
#[test]
fn climbs_page() {
    let file = climbs_route();
    let r = route::Route::parse(&file).unwrap();
    assert_eq!(r.climbs().len(), 3, "{:?}", r.climbs().collect::<Vec<_>>());
    let at = |along_dm: u32, ride: bool| View {
        route: Some(ui::RouteView {
            name: Line::new(format_args!("{}", r.name())),
            length_dm: r.info.length_dm,
            ascent_m: r.info.ascent_m,
            me: None,
            heading_cdeg: None,
            progress: ride.then_some(route::Progress { along_dm, off_dm: 0, segment: 0 }),
        }),
        grade_pct: Some(8),
        vam_m_h: Some(940),
        ..view()
    };
    for (theme, suffix) in [(ui::DAY, ""), (ui::NIGHT, "-night")] {
        for (along_dm, ride, name) in [
            (0, false, "climbs-start"),
            (40_000, true, "climbs-list"),
            (82_000, true, "climbs-in"),
            (155_000, true, "climbs-done"),
        ] {
            let v = at(along_dm, ride);
            let mut ui = Ui::new();
            ui.set_context(true, true, false, ui::Effort::None);
            ui.key(Key::BottomRight, Press::Click, 0, &v);
            ui.tick(0, Some(true), &v);
            while ui.page() != Some(ui::Page::Climbs) {
                ui.key(Key::BottomLeft, Press::Click, 0, &v);
            }
            save_with(&format!("{name}{suffix}"), &ui, &v, &file, &theme);
        }
        // 4 pages on the home screen
        let mut ui = Ui::new();
        ui.set_context(true, true, false, ui::Effort::Power);
        save_with(&format!("climbs-home{suffix}"), &ui, &at(0, false), &file, &theme);
    }
}

/// A distance to a climb is shown only with a current fix close enough to the route to locate the rider on it.
#[test]
fn climb_distances_are_hidden_without_a_confident_gps_position() {
    let file = climbs_route();
    let r = route::Route::parse(&file).unwrap();
    let fix = |lat_e7, lon_e7| {
        Some(ui::nav::Fix {
            lat_e7,
            lon_e7,
            speed_mm_s: Some(3_000),
            course_cdeg: Some(0),
        })
    };
    let mut nav = ui::nav::Nav::default();
    let on_route = nav.update(&r, fix(434_500_000, 20_500_000));
    assert!(on_route.progress.unwrap().off_dm <= ui::OFF_ROUTE_DM);
    let on_route_view = View { route: Some(on_route), ..view() };

    // A lost fix leaves previous route progress available, but it is no longer current.
    let no_fix = nav.update(&r, None);
    assert!(no_fix.progress.is_some());
    let no_fix_view = View {
        gnss: Gnss::Searching { in_view: 4, progress: 50 },
        route: Some(no_fix),
        ..view()
    };

    // About 4 km east of this route: a valid GNSS position, but not one that identifies route progress.
    let off_route = ui::nav::Nav::default().update(&r, fix(434_500_000, 21_000_000));
    assert!(off_route.progress.unwrap().off_dm > ui::OFF_ROUTE_DM);
    let off_route_view = View { route: Some(off_route), ..view() };

    let rendered = |v: &View| {
        let mut ui = Ui::new();
        ui.set_context(true, true, false, ui::Effort::None);
        ui.key(Key::BottomRight, Press::Click, 0, v);
        while ui.page() != Some(ui::Page::Climbs) {
            ui.key(Key::BottomLeft, Press::Click, 0, v);
        }
        screen(&ui, v, &file, &ui::DAY)
    };

    // The next-climb row's right-hand column contains "DANS" and its value only with a trustworthy position.
    let has_distance = |frame: &Fb| {
        (117..164).any(|y| {
            (190..228).any(|x| frame.px[y * frame.w as usize + x] != ui::DAY.next)
        })
    };
    assert!(has_distance(&rendered(&on_route_view)), "on-route fix should show the distance");
    assert!(!has_distance(&rendered(&no_fix_view)), "no current fix must hide the distance");
    assert!(!has_distance(&rendered(&off_route_view)), "off-route fix must hide the distance");
}

/// The climbs page on a representative test route. For each climb, save the list 500 m before its foot,
/// then the climb at 10, 50 and 90 % of it, in `target/tmp/screens/gpx/`.
#[test]
fn climbs_of_a_gpx() {
    let gpx = include_str!("fixtures/climbs.gpx");
    let file = route::build(&route::parse_gpx(gpx).unwrap(), 3.0);
    let r = route::Route::parse(&file).unwrap();
    let mut ui = Ui::new();
    ui.set_context(true, true, false, ui::Effort::None);
    let v = View { grade_pct: None, ..view() };
    ui.key(Key::BottomRight, Press::Click, 0, &v);
    ui.tick(0, Some(true), &v);
    while ui.page() != Some(ui::Page::Climbs) {
        ui.key(Key::BottomLeft, Press::Click, 0, &v);
    }
    for (i, c) in r.climbs().enumerate() {
        println!("climb {}: at {} m, {} m, +{} m", i + 1, c.start_dm / 10, c.length_dm() / 10, c.gain_m);
        let at = |along_dm: u32| View {
            route: Some(ui::RouteView {
                name: Line::new(format_args!("{}", r.name())),
                length_dm: r.info.length_dm,
                ascent_m: r.info.ascent_m,
                me: None,
                heading_cdeg: None,
                progress: Some(route::Progress { along_dm, off_dm: 0, segment: 0 }),
            }),
            ..v
        };
        let before = c.start_dm.saturating_sub(5_000);
        save_with(&format!("gpx/climb{:02}-list", i + 1), &ui, &at(before), &file, &ui::DAY);
        for pct in [10, 50, 90] {
            let along = c.start_dm + c.length_dm() * pct / 100;
            save_with(&format!("gpx/climb{:02}-{pct}", i + 1), &ui, &at(along), &file, &ui::DAY);
        }
    }
}

/// Every top bar the screens can show, one under the other with what it shows, on one sheet
/// (`top-bars.png`, `top-bars-night.png`): the clock, the warnings and the icons side by side at a glance.
#[test]
fn top_bars() {
    use Key::*;
    // the bar and its rule
    const BAR: u32 = 31;
    // a gap, then the label on its own background
    const GAP: u32 = 12;
    const LABEL: u32 = 250;
    let v = view();
    let home = Ui::new();
    let mut ride = Ui::new();
    ride.key(BottomRight, Press::Click, 0, &v);
    ride.tick(0, Some(true), &v);
    let mut paused = ride;
    paused.key(BottomRight, Press::Click, 0, &v);
    let lost = View { gnss: Gnss::Searching { in_view: 3, progress: 0 }, ..v };
    let mut gps_lost = ride;
    gps_lost.tick(1, Some(true), &lost);
    // the map, off the route: the riding view and the screen state at each distance
    let mut map = Ui::new();
    map.set_context(true, false, false, ui::Effort::None);
    let riding = with_route(view(), true);
    map.key(BottomRight, Press::Click, 0, &riding);
    map.tick(0, Some(true), &riding);
    let off = |dm| {
        let mut off = riding;
        if let Some(r) = off.route.as_mut() {
            r.progress = r.progress.map(|p| route::Progress { off_dm: dm, ..p });
        }
        let mut ui = map;
        ui.tick(1, Some(true), &off);
        ui.tick(1 + ui::OFF_ROUTE_ALERT_MS, Some(true), &off);
        (ui, off)
    };
    let (m85, km01, km99, km12) = (off(850), off(1_230), off(99_490), off(123_400));

    let bars: &[(&str, &Ui, View)] = &[
        ("Accueil", &home, v),
        ("Accueil, ni heure ni batterie", &home, View { clock: None, battery_pct: None, ..v }),
        ("Accueil, téléphone", &home, View { phone: true, ..v }),
        ("Accueil, en charge", &home, View { battery_pct: Some(64), plugged: true, ..v }),
        ("Accueil, chargé", &home, View { battery_pct: Some(100), plugged: true, ..v }),
        ("Sortie", &ride, v),
        ("Sortie, téléphone", &ride, View { phone: true, ..v }),
        ("Sortie, tél., 23:59, 100 %", &ride, View { phone: true, clock: Some((23, 59)), battery_pct: Some(100), ..v }),
        ("Sortie, mémoire presque pleine", &ride, View { sparse: true, ..v }),
        ("Sortie, batterie 18 %", &ride, View { battery_pct: Some(18), ..v }),
        ("Sortie, batterie 7 %", &ride, View { battery_pct: Some(7), ..v }),
        ("Sortie, batterie 7 %, en charge", &ride, View { battery_pct: Some(7), plugged: true, ..v }),
        ("Sortie, GPS perdu", &gps_lost, lost),
        ("Sortie, GPS perdu, téléphone", &gps_lost, View { phone: true, ..lost }),
        ("Pause, téléphone", &paused, View { phone: true, ..v }),
        ("Carte, hors trace 85 m, tél.", &m85.0, View { phone: true, ..m85.1 }),
        ("Carte, hors trace 0,1 km, tél.", &km01.0, View { phone: true, ..km01.1 }),
        ("Carte, hors trace 9,9 km", &km99.0, km99.1),
        ("Carte, hors trace 9,9 km, tél.", &km99.0, View { phone: true, ..km99.1 }),
        ("Carte, hors trace 9,9 km, tél., charge", &km99.0, View { phone: true, plugged: true, ..km99.1 }),
        ("Carte, hors trace 12 km, tél.", &km12.0, View { phone: true, ..km12.1 }),
    ];
    for (theme, suffix) in [(ui::DAY, ""), (ui::NIGHT, "-night")] {
        let mut sheet = Fb::new(240 + GAP + LABEL, bars.len() as u32 * (BAR + 4), theme.rule);
        let label = ui::fonts::Style::new(&ui::fonts::BODY, theme.fg, theme.chip);
        for (i, (name, ui, v)) in bars.iter().enumerate() {
            let fb = screen(ui, v, &test_route(), &theme);
            let y0 = i as u32 * (BAR + 4) + 2;
            for y in 0..BAR {
                let row = (y * 240) as usize;
                let at = ((y0 + y) * sheet.w) as usize;
                sheet.px[at..at + 240].copy_from_slice(&fb.px[row..row + 240]);
                sheet.px[at + (240 + GAP) as usize..at + sheet.w as usize].fill(theme.chip);
            }
            ui::fonts::text(&mut sheet, &label, name, (240 + GAP + 8) as i32, (y0 + BAR / 2) as i32, ui::fonts::Align::Left).unwrap();
        }
        sheet.png(&format!("top-bars{suffix}"));
    }
}
