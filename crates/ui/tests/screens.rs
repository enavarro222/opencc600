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
    }
}

fn save(name: &str, ui: &Ui, v: &View, theme: &Theme) {
    let mut fb = Fb(vec![Rgb565::BLACK; 240 * 320]);
    ui::draw(&mut fb, ui, v, theme).unwrap();
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
