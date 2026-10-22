//! The settings (`/settings`, crates `settings`): loaded at boot, changed by the app, the screens or the test hook,
//! saved by the main loop.

use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use log::{info, warn};

use alloc::string::String;

use crate::Fs;

/// Offset from UTC of the local time (the `tz` setting, summer time included) at Unix time `unix` (now when
/// unknown: the rule's offset at 1970, standard time).
pub(crate) fn utc_offset(unix: Option<u32>) -> i32 {
    settings().tz.offset_s(unix.unwrap_or(0) as i64)
}

/// The settings (`/settings`), loaded at boot; changed by the test hook (`set key value`) until the app does it.
pub(crate) static SETTINGS: Mutex<CriticalSectionRawMutex, Cell<Option<settings::Settings>>> =
    Mutex::new(Cell::new(None));

/// A setting changed: to save and apply.
pub(crate) static SETTINGS_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub(crate) const SETTINGS_FILE: &str = "/settings";

pub(crate) fn settings() -> settings::Settings {
    SETTINGS.lock(Cell::get).unwrap_or_default()
}

/// Follows the loaded route, or sets it aside (saved: a ride resumed after a restart keeps it).
pub(crate) fn follow_route(follow: bool) {
    let mut s = settings();
    s.follow_route = follow;
    info!("route: {}", if follow { "followed" } else { "set aside (free ride)" });
    SETTINGS.lock(|c| c.set(Some(s)));
    SETTINGS_CHANGED.signal(());
}

#[allow(
    clippy::large_stack_frames,
    reason = "the file read in a 512-byte buffer, once at boot from main"
)]
pub(crate) fn load_settings(fs: &Fs<'_>) -> settings::Settings {
    let path = littlefs2::path::PathBuf::try_from(SETTINGS_FILE).unwrap();
    match fs.read::<512>(&path) {
        Ok(data) => {
            let s = settings::Settings::parse(core::str::from_utf8(&data).unwrap_or(""));
            info!("settings: {s:?}");
            s
        }
        Err(e) => {
            info!("settings: defaults (no file, littlefs {})", e.code());
            settings::Settings::default()
        }
    }
}

/// The whole file in one commit: a power cut keeps the old one.
pub(crate) fn save_settings(fs: &Fs<'_>, s: &settings::Settings) {
    let mut text = String::new();
    let _ = s.write(&mut text);
    let path = littlefs2::path::PathBuf::try_from(SETTINGS_FILE).unwrap();
    match fs.write(&path, text.as_bytes()) {
        Ok(()) => info!("settings: saved"),
        Err(e) => warn!("settings: save failed, littlefs error {}", e.code()),
    }
}
