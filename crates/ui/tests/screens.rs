//! Draws every screen with sample values: no glyph missing from the fonts (debug builds stop on one), no key label
//! wider than its box, and PNGs to look at in `target/tmp/screens/`.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use ui::{Gnss, Key, Press, RideView, Theme, Ui, View};

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

fn view() -> View {
    View {
        clock: Some((9, 41)),
        battery_pct: Some(81),
        gnss: Gnss::Fix { used: 9 },
        speed_mm_s: Some(9_000),
        altitude_dm: Some(6_604),
        heart_rate: None,
        ride: RideView {
            timer_s: 6128,
            distance_cm: 4_863_000,
            ascent_m: 812,
            descent_m: 790,
            max_speed_mm_s: 17_400,
        },
        rides_stored: Some(12),
        free_hours: Some(38),
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

#[test]
fn every_screen() {
    use Key::*;
    let v = view();
    for (theme, suffix) in [(ui::DAY, "day"), (ui::NIGHT, "night")] {
        let mut ui = Ui::new();
        let shot = |ui: &Ui, name: &str| save(&format!("{name}-{suffix}"), ui, &v, &theme);
        shot(&ui, "1-home");
        ui.key(BottomLeft, Press::Click, 0);
        shot(&ui, "2-status");
        ui.key(TopLeft, Press::Click, 0);
        ui.key(BottomRight, Press::Click, 0);
        shot(&ui, "3-ride-page1");
        ui.key(BottomLeft, Press::Click, 0);
        shot(&ui, "4-ride-page2");
        ui.key(BottomRight, Press::Click, 0);
        shot(&ui, "5-paused");
        ui.key(BottomLeft, Press::Click, 0);
        shot(&ui, "6-confirm-stop");
        ui.key(BottomRight, Press::Click, 0);
        shot(&ui, "7-saved");
        ui.key(BottomRight, Press::Click, 0);
        ui.key(TopLeft, Press::Long, 0);
        shot(&ui, "8-power-off");
        ui.key(BottomLeft, Press::Click, 0);
        ui.set_notice(ui::NOTICE_NO_TIME, 0);
        shot(&ui, "9-notice");
    }
    // unknown values
    let empty = View::default();
    let mut ui = Ui::new();
    save("10-home-empty", &ui, &empty, &ui::DAY);
    ui.key(Key::BottomRight, Press::Click, 0);
    save("11-ride-empty", &ui, &empty, &ui::DAY);
    // a slow speed keeps the comma of 32,4 (right-aligned in the slot of "00,0")
    let mut ui = Ui::new();
    ui.key(Key::BottomRight, Press::Click, 0);
    let slow = View { speed_mm_s: Some(2_722), battery_pct: Some(9), ..view() };
    save("13-ride-slow", &ui, &slow, &ui::DAY);
    let searching = View {
        gnss: Gnss::Searching { in_view: 12 },
        ..empty
    };
    save("12-home-searching", &Ui::new(), &searching, &ui::DAY);
}
