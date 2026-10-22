//! The route (`/route`, crates `route`): stored, loaded at boot, and the background map benchmark.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Instant;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use log::{info, warn};
use opencc600_firmware::panel::FrameBuffer;

use alloc::vec::Vec;

use crate::Fs;
use crate::screen::theme;

/// The route file, and the one being written (renamed over it once complete: a power cut keeps one of the two).
pub(crate) const ROUTE_FILE: &str = "/route";

pub(crate) const ROUTE_NEW: &str = "/route.new";

/// Largest route accepted (~9 000 points).
pub(crate) const ROUTE_MAX: usize = 64 * 1024;

/// A route received (`None`: remove the route).
pub(crate) static ROUTE_RECEIVED: Signal<CriticalSectionRawMutex, Option<Vec<u8>>> = Signal::new();

#[allow(
    clippy::large_stack_frames,
    reason = "littlefs file handle, once at boot from main"
)]
pub(crate) fn load_route(fs: &Fs<'_>) -> Option<Vec<u8>> {
    let path = littlefs2::path::PathBuf::try_from(ROUTE_FILE).unwrap();
    let data = fs
        .open_file_and_then(&path, |file| {
            let mut data = alloc::vec![0; file.len()?.min(ROUTE_MAX)];
            let mut n = 0;
            while n < data.len() {
                match file.read(&mut data[n..])? {
                    0 => break,
                    k => n += k,
                }
            }
            data.truncate(n);
            Ok(data)
        })
        .ok()?;
    match route::Route::parse(&data) {
        Ok(r) => {
            info!(
                "route: {:?}, {} points, {} m",
                r.name(),
                r.info.count,
                r.info.length_dm / 10
            );
            Some(data)
        }
        Err(e) => {
            warn!("route: {ROUTE_FILE} unreadable ({e:?})");
            None
        }
    }
}

/// Checks a route received and makes it the route (`None`: removes the route); returns the route now.
#[allow(
    clippy::large_stack_frames,
    reason = "littlefs file handles, once a route from main"
)]
pub(crate) fn store_route(fs: &Fs<'_>, data: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let path = littlefs2::path::PathBuf::try_from(ROUTE_FILE).unwrap();
    let Some(data) = data else {
        let _ = fs.remove(&path);
        info!("route: removed");
        return None;
    };
    if let Err(e) = route::Route::parse(&data) {
        warn!("route: received file refused ({e:?}), the route stays as it was");
        return load_route(fs);
    }
    let new = littlefs2::path::PathBuf::try_from(ROUTE_NEW).unwrap();
    match fs.write(&new, &data).and_then(|()| fs.rename(&new, &path)) {
        Ok(()) => {
            let r = route::Route::parse(&data).unwrap();
            info!(
                "route: loaded {:?}, {} points, {} m",
                r.name(),
                r.info.count,
                r.info.length_dm / 10
            );
            Some(data)
        }
        Err(e) => {
            warn!("route: write failed, littlefs error {}", e.code());
            load_route(fs)
        }
    }
}

/// The background map built in (feature `bench-map`), if it was made for this route.
pub(crate) fn map_for(route_file: Option<&[u8]>) -> Option<route::map::Map<'static>> {
    #[cfg(feature = "bench-map")]
    const FILE: &[u8] = include_bytes!("../../../dump/bench.map");
    #[cfg(not(feature = "bench-map"))]
    const FILE: &[u8] = &[];
    let route_crc = route::crc16(route_file?);
    match route::map::Map::parse(FILE) {
        Ok(m) if m.route_crc == route_crc => {
            info!("map: built-in, {} bytes", FILE.len());
            Some(m)
        }
        Ok(_) => {
            warn!("map: built-in one made for another route");
            None
        }
        Err(_) => None,
    }
}

/// The map page drawn on the route, at each zoom, with the background map and without (the test hook's
/// `bench map`).
#[allow(
    clippy::large_stack_frames,
    reason = "a view and a Ui copy, from main, on request of the test hook"
)]
pub(crate) fn bench_map(fb: &mut FrameBuffer, r: &route::Route<'_>, m: Option<&route::map::Map<'_>>, view: &ui::View) {
    let p = r.points().nth(r.info.count as usize / 3).unwrap();
    let mut ui = ui::Ui::new();
    ui.set_context(true, false, false, ui::Effort::None);
    let mut v = *view;
    v.route = Some(ui::RouteView {
        name: ui::fonts::Line::new(format_args!("{}", r.name())),
        length_dm: r.info.length_dm,
        ascent_m: r.info.ascent_m,
        me: Some((p.x, p.y)),
        heading_cdeg: Some(4_500),
        progress: Some(r.locate(p.x, p.y, None)),
    });
    ui.key(ui::Key::BottomRight, ui::Press::Click, 0, &v);
    for _ in 0..ui::ZOOM_STEPS {
        let t = Instant::now();
        ui::draw(fb, &ui, &v, Some(r), None, &theme()).unwrap();
        let draw = t.elapsed().as_millis();
        let t = Instant::now();
        ui::draw(fb, &ui, &v, Some(r), m, &theme()).unwrap();
        let with_map = t.elapsed().as_millis();
        let t = Instant::now();
        let fix = Some(ui::nav::Fix {
            lat_e7: r.info.lat0_e7,
            lon_e7: r.info.lon0_e7,
            speed_mm_s: Some(5_000),
            course_cdeg: Some(0),
        });
        let _ = ui::nav::Nav::default().update(r, fix);
        let locate = t.elapsed().as_millis();
        info!(
            "bench: map at {:?} m drawn in {draw} ms, {with_map} ms with the background, located in {locate} ms",
            ui.zoom_m()
        );
        ui.key(ui::Key::TopLeft, ui::Press::Click, 0, &v);
    }
    // what the rest costs: the map without the route, the Essential page, a clear
    let t = Instant::now();
    ui::draw(fb, &ui, &v, None, None, &theme()).unwrap();
    let no_route = t.elapsed().as_millis();
    ui.key(ui::Key::BottomLeft, ui::Press::Click, 0, &v);
    let t = Instant::now();
    ui::draw(fb, &ui, &v, Some(r), None, &theme()).unwrap();
    let essential = t.elapsed().as_millis();
    let t = Instant::now();
    fb.clear(Rgb565::WHITE).unwrap();
    let clear = t.elapsed().as_micros();
    info!(
        "bench: map page without the route {no_route} ms, Essential page {essential} ms, clear {clear} us"
    );
}
