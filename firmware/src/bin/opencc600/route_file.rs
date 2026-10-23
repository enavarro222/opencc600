//! The route (`/route`, crates `route`) and its background map (`/map`): stored, loaded at boot, and the background
//! map benchmark.

use core::sync::atomic::{AtomicU32, Ordering};

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

/// A whole file, up to `max` bytes; `None` when there is none or it cannot be read.
#[allow(
    clippy::large_stack_frames,
    reason = "littlefs file handle, from main: at boot and when a route comes"
)]
fn read_file(fs: &Fs<'_>, name: &str, max: usize) -> Option<Vec<u8>> {
    let path = littlefs2::path::PathBuf::try_from(name).unwrap();
    fs.open_file_and_then(&path, |file| {
        let mut data = alloc::vec![0; file.len()?.min(max)];
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
    .ok()
}

pub(crate) fn load_route(fs: &Fs<'_>) -> Option<Vec<u8>> {
    let data = read_file(fs, ROUTE_FILE, ROUTE_MAX)?;
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

/// The background map of the route, and the one being written (renamed over it once complete).
pub(crate) const MAP_FILE: &str = "/map";

pub(crate) const MAP_NEW: &str = "/map.new";

/// Largest map accepted: it is held whole in the PSRAM (2 MB: the framebuffer takes 150 KB, and a new map arrives
/// while the old one is still shown). ~300 KB for 100 km with every detail outside cities.
pub(crate) const MAP_MAX: usize = 512 * 1024;

/// Flash a new map leaves free for the rides: ~6 hours of riding.
pub(crate) const MAP_LEAVES_FREE: usize = 1024 * 1024;

/// A map received, checked by the link (`companion`).
pub(crate) static MAP_RECEIVED: Signal<CriticalSectionRawMutex, Vec<u8>> = Signal::new();

/// Bytes of `/map`: a new map takes its place.
pub(crate) static MAP_BYTES: AtomicU32 = AtomicU32::new(0);

/// The map stored, if it is the map of this route (the background map built in with the feature `bench-map`, when
/// none is stored). A map for another route is removed (its room is the next one's), read no further than its
/// header.
#[allow(
    clippy::large_stack_frames,
    reason = "littlefs file handles, from main: at boot and when a route or a map comes"
)]
pub(crate) fn load_map(fs: &Fs<'_>, route_file: Option<&[u8]>) -> Option<Vec<u8>> {
    let path = littlefs2::path::PathBuf::try_from(MAP_FILE).unwrap();
    let size = fs.metadata(&path).map_or(0, |m| m.len());
    MAP_BYTES.store(size as u32, Ordering::Relaxed);
    // no route, no map to show: its room goes back to the rides
    let Some(route_file) = route_file else {
        if size > 0 {
            let _ = fs.remove(&path);
            MAP_BYTES.store(0, Ordering::Relaxed);
        }
        return None;
    };
    #[cfg(feature = "bench-map")]
    if size == 0 {
        return map_of(route_file, include_bytes!("../../../dump/bench.map").to_vec()).ok();
    }
    if size == 0 {
        return None;
    }
    let head = read_file(fs, MAP_FILE, 7)?;
    if route::map::route_crc_of(&head) != Some(route::crc16(route_file)) {
        info!("map: made for another route, removed");
        let _ = fs.remove(&path);
        MAP_BYTES.store(0, Ordering::Relaxed);
        return None;
    }
    map_of(route_file, read_file(fs, MAP_FILE, MAP_MAX)?)
        .inspect_err(|why| warn!("map: {why}"))
        .ok()
}

/// The map, if it was made for this route; else why not, for the screen.
fn map_of(route_file: &[u8], data: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    match route::map::Map::parse(&data) {
        Ok(m) if m.route_crc == route::crc16(route_file) => {
            info!("map: {} bytes", data.len());
            Ok(data)
        }
        Ok(_) => Err("Faite pour une autre trace"),
        Err(_) => Err("Fichier abîmé"),
    }
}

/// Stores a map received when it is the route's (checked first: the flash is not spent on another one); returns it,
/// or why it was not kept. The caller lets go of the map it shows before: two maps at once may not fit the PSRAM.
#[allow(
    clippy::large_stack_frames,
    reason = "littlefs file handles, once a map from main"
)]
pub(crate) fn store_map(fs: &Fs<'_>, data: Vec<u8>, route_file: Option<&[u8]>) -> Result<Vec<u8>, &'static str> {
    let data = map_of(route_file.ok_or("Pas de trace sur le compteur")?, data)?;
    let path = littlefs2::path::PathBuf::try_from(MAP_FILE).unwrap();
    let new = littlefs2::path::PathBuf::try_from(MAP_NEW).unwrap();
    match fs.write(&new, &data).and_then(|()| fs.rename(&new, &path)) {
        Ok(()) => {
            MAP_BYTES.store(data.len() as u32, Ordering::Relaxed);
            Ok(data)
        }
        Err(e) => {
            warn!("map: write failed, littlefs error {}", e.code());
            let _ = fs.remove(&new);
            Err("Mémoire pleine")
        }
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
