//! What the screens show (crates/ui) and the bring-up screens: the view, the theme, the status screen, the panel
//! benchmark.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_time::Instant;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_10X20};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::{Alignment, Text};
use esp_hal::time as hal_time;
use log::info;
use nrf_link::KeyAction;
use opencc600_firmware::ble;
use opencc600_firmware::panel::{self, FrameBuffer, Panel};
use opencc600_firmware::storage;
use status::Status;

use alloc::format;
use alloc::string::String;

use crate::config::{settings, utc_offset};
use crate::nrf::stream_lost;
use crate::pairing::{POWER_3S, sensor_values, sensors_view};
use crate::ride::RideInfo;
use crate::trip_log::TRIP_LOG_BYTES_PER_S;
use crate::usb::{FAKE_BATTERY, FAKE_NONE};

/// The night theme is shown: the `theme` setting, or from sunset to sunrise with `auto_night`.
pub(crate) static NIGHT: AtomicBool = AtomicBool::new(false);

/// The backlight set, percent.
pub(crate) static BACKLIGHT: AtomicU8 = AtomicU8::new(0);

pub(crate) fn theme() -> ui::Theme {
    if NIGHT.load(Ordering::Relaxed) {
        ui::NIGHT
    } else {
        ui::DAY
    }
}

/// What the screens show (crates/ui).
#[allow(
    clippy::large_stack_frames,
    reason = "the view (two altitude profiles) and the status, from main"
)]
pub(crate) fn view_of(s: &Status, ride: &RideInfo, resumable: bool) -> ui::View {
    let fix = s.fix;
    let altitude_dm = s.altimeter.altitude_dm();
    let sensors = sensor_values();
    ui::View {
        clock: s.now_unix().map(|unix| {
            let day_s = (unix as i64 + utc_offset(Some(unix)) as i64).rem_euclid(86_400) as u32;
            ((day_s / 3600) as u8, (day_s / 60 % 60) as u8)
        }),
        battery_pct: match FAKE_BATTERY.load(Ordering::Relaxed) {
            FAKE_NONE => s.battery.map(|(_, pct)| pct),
            pct => Some(pct),
        },
        plugged: s.charger == Some(nrf_link::USB_POWERED),
        phone: ble::app_linked(),
        stream_lost: stream_lost(),
        write_failure: ride.failure,
        sparse: ride.sparse,
        rides: ui::RidesView::default(),
        gnss: match fix {
            None => ui::Gnss::Off,
            Some(f) if f.valid => ui::Gnss::Fix { used: f.sats_used },
            Some(f) => ui::Gnss::Searching {
                in_view: f.sats_in_view,
                // a guess: ~45 s to a fix with the RTC time (dump/ttff.csv), never full before the fix
                progress: (Instant::now().as_secs() * 100 / 45).min(95) as u8,
            },
        },
        // the receiver reports a few tenths of m/s at rest
        speed_mm_s: fix
            .filter(|f| f.valid)
            .and_then(|f| f.speed_mm_s)
            .map(|v| if v < recorder::MOVING_MM_S { 0 } else { v }),
        altitude_dm,
        grade_pct: altitude_dm.and_then(|a| ride.history.grade_pct(ride.distance_cm, a)),
        vam_m_h: ride.history.vam_m_h(),
        heart_rate: sensors.heart_rate,
        cadence_rpm: sensors.cadence_rpm,
        power_w: sensors.power_w,
        power_3s_w: POWER_3S.lock(|c| {
            let now = Instant::now().as_millis();
            let mut a = c.get();
            a.push(sensors.power_w, now);
            c.set(a);
            a.get(now)
        }),
        wheel_speed_mm_s: sensors.speed_mm_s,
        ftp_w: settings().ftp_w,
        hr_max: settings().hr_max,
        power_zones: settings().power_zones,
        hr_zones: settings().hr_zones,
        sensors: sensors_view(),
        ride: ui::RideView {
            timer_s: ride.timer_s,
            elapsed_s: ride.elapsed_s,
            distance_cm: ride.distance_cm,
            ascent_m: ride.ascent_m,
            descent_m: ride.descent_m,
            max_speed_mm_s: ride.max_speed_mm_s,
            avg_power_w: ride.avg_power_w,
            avg_heart_rate: ride.avg_heart_rate,
            max_heart_rate: ride.max_heart_rate,
        },
        recent: ride.history.recent(),
        whole: ride.history.whole(),
        rides_stored: s.storage.and_then(|r| r.ok()).map(|(rides, _, _)| rides),
        saving: false,
        record_hours: s.storage.and_then(|r| r.ok()).map(|(_, free, _)| {
            let log = if settings().trip_log { TRIP_LOG_BYTES_PER_S } else { 0 };
            recorder::hours_left(free as u64 * storage::BLOCK as u64, log)
        }),
        resumable,
        route: None,
        route_set_aside: false,
    }
}

/// Centred box over the current screen.
pub(crate) fn draw_message(fb: &mut FrameBuffer, text: &str) {
    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    Rectangle::new(Point::new(20, 100), Size::new(200, 120))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(Rgb565::CSS_DARK_RED)
                .stroke_color(Rgb565::WHITE)
                .stroke_width(2)
                .build(),
        )
        .draw(fb)
        .unwrap();
    Text::with_alignment(text, Point::new(120, 140), style, Alignment::Center)
        .draw(fb)
        .unwrap();
}

#[allow(
    clippy::large_stack_frames,
    reason = "bring-up screen, called from main, ~1.5 KB"
)]
/// The bring-up values, in the content area of the status screen (between the key bars of crates/ui).
pub(crate) fn draw_status(fb: &mut FrameBuffer, s: &Status, ride: Option<RideInfo>) {
    let label = MonoTextStyle::new(&FONT_6X10, theme().mute);
    let value = MonoTextStyle::new(&FONT_10X20, theme().fg);

    let none = || String::from("-");
    let rows: [(&str, String); 9] = [
        (
            "ride: timer, km (key 1: start/pause, long: stop)",
            ride.map_or_else(none, |r| {
                let t = r.timer_s;
                format!(
                    "{} {}:{:02}:{:02} {}.{:02}km",
                    if r.running { "REC" } else { "PAUSE" },
                    t / 3600,
                    t / 60 % 60,
                    t % 60,
                    r.distance_cm / 100_000,
                    r.distance_cm / 1000 % 100
                )
            }),
        ),
        (
            "GNSS fix: sats used/in view, hdop, km/h",
            s.fix.map_or_else(none, |f| {
                if f.valid {
                    format!(
                        "{}/{} {}.{:02} {}",
                        f.sats_used,
                        f.sats_in_view,
                        f.hdop_c.unwrap_or(9999) / 100,
                        f.hdop_c.unwrap_or(9999) % 100,
                        f.speed_mm_s.unwrap_or(0) * 36 / 10_000
                    )
                } else {
                    format!("no fix ({} in view)", f.sats_in_view)
                }
            }),
        ),
        (
            "GNSS time (UTC) / link errors",
            s.fix.and_then(|f| f.time).map_or_else(none, |t| {
                let errors = s.gnss.map_or(0, |g| g.bad_checksum + g.cut + g.malformed);
                format!("{:02}:{:02}:{:02} / {errors}", t.hour, t.minute, t.second)
            }),
        ),
        (
            "storage: rides / free blocks",
            match s.storage {
                None => none(),
                Some(Ok((rides, free, total))) => format!("{rides} / {free} of {total}"),
                Some(Err(e)) => format!("error {e}"),
            },
        ),
        (
            "battery / charger status",
            format!(
                "{} / {}",
                s.battery
                    .map_or_else(none, |(mv, pct)| format!("{pct}% {mv}mV")),
                s.charger.map_or_else(none, |c| format!("{c}"))
            ),
        ),
        (
            "nRF RTC (UTC)",
            s.rtc.map_or_else(none, |t| {
                format!(
                    "{:02}/{:02} {:02}:{:02}:{:02}",
                    t.day, t.month, t.hour, t.minute, t.second
                )
            }),
        ),
        (
            "last key (events)",
            format!(
                "{} ({})",
                s.last_key.map_or_else(none, |(k, a)| match a {
                    KeyAction::Click => format!("{k} click"),
                    KeyAction::LongPress => format!("{k} long"),
                    KeyAction::LongRelease => format!("{k} release"),
                    KeyAction::Other(v) => format!("{k} ?{v}"),
                }),
                s.keys
            ),
        ),
        (
            "baro: temperature, pressure, altitude",
            s.baro.map_or_else(
                || String::from("- (long press key 0)"),
                |(t, p)| {
                    format!(
                        "{}.{:01}C {}.{:01}hPa {}",
                        t / 100,
                        (t % 100).abs() / 10,
                        p / 10_000,
                        p / 1000 % 10,
                        s.altimeter
                            .altitude_dm()
                            .map_or_else(|| String::from("-"), |a| format!("{}m", a / 10))
                    )
                },
            ),
        ),
        ("light raw (600/2000/3000)", {
            let l = |v: Option<u16>| v.map_or_else(|| String::from("-"), |l| format!("{l}"));
            format!("{} / {} / {}", l(s.light[0]), l(s.light[1]), l(s.light[2]))
        }),
    ];
    for (i, (name, v)) in rows.iter().enumerate() {
        let y = ui::CONTENT.top_left.y + 10 + i as i32 * 27;
        Text::new(name, Point::new(8, y), label).draw(fb).unwrap();
        Text::new(v, Point::new(8, y + 20), value).draw(fb).unwrap();
    }
}

/// Checks orientation (red square top-left), colour order (bars), data lines (smooth 32-step gradients per
/// channel: a swapped or stuck bus line breaks the ramp) and offsets (1 px white border).
#[allow(clippy::large_stack_frames, reason = "one-off at boot, ~1 KB")]
pub(crate) fn draw_test_pattern(fb: &mut FrameBuffer) {
    let fill = |c| PrimitiveStyle::with_fill(c);
    fb.clear(Rgb565::BLACK).unwrap();
    Rectangle::new(Point::zero(), Size::new(240, 320))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
        .draw(fb)
        .unwrap();
    Rectangle::new(Point::new(4, 4), Size::new(24, 24))
        .into_styled(fill(Rgb565::RED))
        .draw(fb)
        .unwrap();

    let bars = [
        Rgb565::RED,
        Rgb565::GREEN,
        Rgb565::BLUE,
        Rgb565::WHITE,
        Rgb565::YELLOW,
        Rgb565::CYAN,
    ];
    for (i, c) in bars.into_iter().enumerate() {
        Rectangle::new(Point::new(i as i32 * 40, 80), Size::new(40, 60))
            .into_styled(fill(c))
            .draw(fb)
            .unwrap();
    }

    // 32 steps per channel (green: 64 levels, sampled every other level)
    for step in 0..32u8 {
        let x = 8 + step as i32 * 7;
        for (row, c) in [
            Rgb565::new(step, 0, 0),
            Rgb565::new(0, step * 2, 0),
            Rgb565::new(0, 0, step),
        ]
        .into_iter()
        .enumerate()
        {
            Rectangle::new(Point::new(x, 160 + row as i32 * 30), Size::new(7, 26))
                .into_styled(fill(c))
                .draw(fb)
                .unwrap();
        }
    }

    let text = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    Text::with_alignment("opencc600", Point::new(120, 60), text, Alignment::Center)
        .draw(fb)
        .unwrap();
    Text::with_alignment("R G B W Y C", Point::new(120, 270), text, Alignment::Center)
        .draw(fb)
        .unwrap();
}

/// Display timings, logged once at boot. Theory at 15 MHz, one pixel per clock: 76 800 px = 5.12 ms per frame.
pub(crate) fn bench(panel: &mut Panel, fb: &mut FrameBuffer) {
    const N: u32 = 20;
    let log = |what: &str, total: hal_time::Duration, s: panel::Stats| {
        info!(
            "bench {what}: {} us/frame (fill {} us, transfer {} us, {} transfers)",
            total.as_micros() / N as u64,
            s.fill.as_micros() / N as u64,
            s.transfer.as_micros() / N as u64,
            s.transfers / N,
        )
    };

    let t = hal_time::Instant::now();
    for _ in 0..N {
        fb.clear(Rgb565::BLACK).unwrap();
    }
    info!(
        "bench framebuffer clear (PSRAM): {} us",
        t.elapsed().as_micros() / N as u64
    );

    let t = hal_time::Instant::now();
    for _ in 0..N {
        draw_test_pattern(fb);
    }
    info!(
        "bench draw test pattern (PSRAM): {} us",
        t.elapsed().as_micros() / N as u64
    );

    panel.take_stats();
    let t = hal_time::Instant::now();
    for i in 0..N {
        panel.clear(if i % 2 == 0 {
            Rgb565::BLUE
        } else {
            Rgb565::BLACK
        });
    }
    log("solid fill", t.elapsed(), panel.take_stats());

    let t = hal_time::Instant::now();
    for _ in 0..N {
        panel.flush(fb);
    }
    log("flush framebuffer", t.elapsed(), panel.take_stats());
}
