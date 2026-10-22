//! The test hook: the commands typed on the USB serial port (crates/test-hook), and what they change.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::usb::usb_serial_jtag::UsbSerialJtagRx;
use log::{info, warn};
use nrf_link::{RtcTime, command};
use opencc600_firmware::{ant, ble};
use opencc600_firmware::storage;
use ride_store::room;
use status::Status;
use test_hook::Command;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::config::{SETTINGS, SETTINGS_CHANGED, settings};
use crate::{Fs, STATUS, STATUS_CHANGED};
use crate::nrf::{NRF_COMMANDS, NRF_KEYS, SOUND_ON, log_key};
use crate::receiver::{GNSS_POWER_LOST, GNSS_REQUESTS, GnssRequest, SIM_FIX_AT, sim_fix_active};
use crate::route_file::{ROUTE_MAX, ROUTE_RECEIVED};
use crate::screen::{BACKLIGHT, NIGHT};

/// An ambient light level from USB (`light <raw>`), in place of the sensor's for the automatic backlight, until
/// `light off` or a reset.
pub(crate) static SIM_LIGHT: Mutex<CriticalSectionRawMutex, Cell<Option<u16>>> = Mutex::new(Cell::new(None));

/// A route being received over USB.
pub(crate) static ROUTE_RX: Mutex<CriticalSectionRawMutex, RefCell<Option<Vec<u8>>>> =
    Mutex::new(RefCell::new(None));

/// The test hook asks for the map benchmark.
pub(crate) static BENCH_MAP: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The test hook fills the storage (`storage fill N`: the logs deleted, until N blocks are left free) or empties it
/// again (`storage unfill`, `None`), with [`FILL_FILE`] only.
pub(crate) static STORAGE_FILL: Signal<CriticalSectionRawMutex, Option<u16>> = Signal::new();

pub(crate) const FILL_FILE: &str = "/fill.bin";

/// Blocks written into [`FILL_FILE`] per pass of the main loop (~1.3 s: 80 ms a block, measured).
pub(crate) const FILL_STEP: usize = 16;

/// Starts a `storage fill` (removes the file first): the blocks it leaves free.
#[allow(clippy::large_stack_frames, reason = "littlefs path and file handle on the main stack, a test hook")]
pub(crate) fn storage_fill_start(fs: &Fs<'_>, fill: Option<u16>) -> Option<usize> {
    let path = littlefs2::path::PathBuf::try_from(FILL_FILE).unwrap();
    if fs.exists(&path)
        && let Err(e) = fs.remove(&path)
    {
        warn!("storage fill: littlefs error {}", e.code());
    }
    let Some(left) = fill else {
        info!("storage fill: removed");
        return None;
    };
    // the logs first, as a ride short of room would: what is left free is all the room there is
    if let Err(e) = room::trim_logs(fs, 0, usize::MAX) {
        warn!("storage fill: littlefs error {} deleting the logs", e.code());
    }
    info!("storage fill: until {left} blocks are left free, logs deleted");
    fs.write(&path, &[]).ok().map(|()| usize::from(left))
}

/// Writes the next blocks of a `storage fill` (the free blocks read again each time: the file's own block pointers
/// take some too); `None` once done.
#[allow(clippy::large_stack_frames, reason = "littlefs path and file handle on the main stack, a test hook")]
pub(crate) fn storage_fill(fs: &Fs<'_>, left: usize) -> Option<usize> {
    let path = littlefs2::path::PathBuf::try_from(FILL_FILE).unwrap();
    // one block for the pointers and the commit
    let n = fs.available_blocks().unwrap_or(0).saturating_sub(left + 1).min(FILL_STEP);
    let chunk = [0x55u8; 256];
    let res = fs.open_file_with_options_and_then(
        |o| o.write(true).append(true),
        &path,
        |file| {
            for _ in 0..n * storage::BLOCK / chunk.len() {
                file.write(&chunk)?;
            }
            Ok(())
        },
    );
    match res {
        Ok(()) if n == 0 => {
            info!("storage fill: done, {} blocks free", fs.available_blocks().unwrap_or(0));
            None
        }
        Ok(()) => Some(left),
        Err(e) => {
            info!("storage fill: done, littlefs error {}", e.code());
            None
        }
    }
}

/// Test hook (`power off dry`, until the next boot): a power off restarts the ESP32 instead of asking the nRF to cut
/// it, which would stop the nRF's sensor stream (time, barometer) until a long press of the power key.
pub(crate) static DRY_POWER_OFF: AtomicBool = AtomicBool::new(false);

/// Test hook (`battery fake N`, `battery real`): the battery level shown, not the one measured (the charging screen's
/// animation at full charge).
pub(crate) static FAKE_BATTERY: AtomicU8 = AtomicU8::new(FAKE_NONE);

pub(crate) const FAKE_NONE: u8 = u8::MAX;

/// The screen shown, for the test hook, with what is selected on it: the menu entry, the sensor role, the entry
/// of the scan screen, the ride page.
pub(crate) type Shown = (ui::Screen, Option<ui::Item>, ui::Role, u8, Option<ui::Page>);

pub(crate) static SCREEN: Mutex<CriticalSectionRawMutex, Cell<Shown>> =
    Mutex::new(Cell::new((ui::Screen::Home, None, ui::Role::HeartRate, 0, None)));

/// The ride selected on the rides screen (its id, 0 for none), for the test hook: a test deletes its own ride only.
pub(crate) static SHOWN_RIDE: AtomicU32 = AtomicU32::new(0);

/// The test hook: the commands typed on the USB serial port (crates/test-hook).
#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: line buffer, statically allocated by embassy"
)]
pub(crate) async fn usb_keys(mut rx: UsbSerialJtagRx<'static, esp_hal::Blocking>) {
    // room for `route data` and 100 bytes in hex, after a sequence number
    let mut line = [0u8; 256];
    let mut len = 0;
    let mut last_seq = None;
    loop {
        let mut buf = [0u8; 64];
        let n = rx.drain_rx_fifo(&mut buf);
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                let text = core::str::from_utf8(&line[..len]).unwrap_or("");
                let (seq, text) = test_hook::split_seq(text);
                let command = test_hook::parse(text);
                if command == Some(Command::RouteDataDamaged) {
                    warn!("usb: route data line damaged, not acknowledged");
                    len = 0;
                    continue;
                }
                if let Some(seq) = seq {
                    // acknowledged again when repeated (the host resends until it sees the ack), run once; only
                    // the last number counts: a new host session starts again from 1
                    info!("usb ack {seq}");
                    if last_seq == Some(seq) {
                        len = 0;
                        continue;
                    }
                    last_seq = Some(seq);
                }
                match command {
                    Some(Command::Key(key, actions)) => {
                        info!("usb: key {key} {actions:?}");
                        for &action in actions {
                            log_key(key, action, true);
                            NRF_KEYS.send((key, action)).await;
                        }
                    }
                    Some(Command::Sound(on)) => {
                        info!("usb: sound {}", if on { "on" } else { "off" });
                        SOUND_ON.store(on, Ordering::Relaxed);
                    }
                    Some(Command::GnssStart(start)) => GNSS_REQUESTS.send(GnssRequest::Start(start)).await,
                    Some(Command::GnssTimeAiding(on)) => GNSS_REQUESTS.send(GnssRequest::TimeAiding(on)).await,
                    Some(Command::Screen) => match SCREEN.lock(Cell::get) {
                        (s @ (ui::Screen::Menu | ui::Screen::PauseMenu), Some(item), ..) => {
                            info!("screen {s:?} {item:?}")
                        }
                        (s @ ui::Screen::Sensors, _, role, ..) => info!("screen {s:?} {role:?}"),
                        (s @ ui::Screen::SensorScan, _, role, i, _) => info!("screen {s:?} {role:?} {i}"),
                        (s @ ui::Screen::Ride, .., Some(page)) => info!("screen {s:?} {page:?}"),
                        (s @ (ui::Screen::Rides | ui::Screen::ConfirmDelete), ..) => {
                            info!("screen {s:?} {}", SHOWN_RIDE.load(Ordering::Relaxed))
                        }
                        (s, ..) => info!("screen {s:?}"),
                    },
                    Some(Command::BleScan(seconds)) => ble::SCAN.signal(seconds),
                    Some(Command::NrfE2(grp, id, val)) => {
                        info!("usb: nrf E2 {grp:02X} {id:02X} 00 00 {val:02X} 00 00");
                        NRF_COMMANDS.send(command::e2(grp, id, val)).await;
                    }
                    Some(Command::NrfRaw(kind, cmd, payload)) => {
                        info!("usb: nrf {kind} {cmd:02X} {payload:02X?}");
                        NRF_COMMANDS.send(command::raw(kind, cmd, payload)).await;
                    }
                    Some(Command::AntScan(seconds)) => {
                        info!("usb: ant scan {seconds} s");
                        NRF_COMMANDS.send(ant::search(seconds, seconds > 0)).await;
                    }
                    Some(Command::FakeBattery(pct)) => {
                        info!("usb: battery shown {}", pct.map_or_else(|| String::from("real"), |p| format!("{p} %")));
                        FAKE_BATTERY.store(pct.unwrap_or(FAKE_NONE), Ordering::Relaxed);
                        STATUS_CHANGED.signal(());
                    }
                    Some(Command::DryPowerOff) => {
                        info!("usb: power off dry");
                        DRY_POWER_OFF.store(true, Ordering::Relaxed);
                    }
                    Some(Command::GnssOff(lost)) => {
                        info!("usb: gnss off, {lost} power-ons lost");
                        GNSS_POWER_LOST.store(lost, Ordering::Relaxed);
                        NRF_COMMANDS.send(command::gps_power(0)).await;
                    }
                    Some(Command::Set(key, value)) => {
                        let mut s = settings();
                        match s.set(key, value) {
                            Ok(()) => {
                                SETTINGS.lock(|c| c.set(Some(s)));
                                SETTINGS_CHANGED.signal(());
                                info!("usb: set {key}={value}");
                            }
                            Err(e) => warn!("usb: set {key}={value}: {e:?}"),
                        }
                    }
                    Some(Command::Get(key)) => {
                        let mut v = String::new();
                        match settings().get(key, &mut v) {
                            Ok(()) => info!("usb: {key}={v}"),
                            Err(e) => warn!("usb: get {key}: {e:?}"),
                        }
                    }
                    Some(Command::Settings) => {
                        let mut v = String::new();
                        let _ = settings().write(&mut v);
                        for line in v.lines() {
                            info!("usb: {line}");
                        }
                    }
                    Some(Command::RouteBegin(n)) if n <= ROUTE_MAX => {
                        ROUTE_RX.lock(|r| *r.borrow_mut() = Some(Vec::with_capacity(n)));
                        info!("usb: route begin, {n} bytes");
                    }
                    Some(Command::RouteBegin(n)) => {
                        warn!("usb: route of {n} bytes, more than {ROUTE_MAX}")
                    }
                    Some(Command::RouteData(data)) => {
                        let ok = ROUTE_RX.lock(|r| match r.borrow_mut().as_mut() {
                            Some(buf) if buf.len() + data.len() / 2 <= ROUTE_MAX => {
                                test_hook::unhex(data, buf)
                            }
                            _ => false,
                        });
                        if !ok {
                            warn!("usb: route data refused (no begin, too long or not hex)");
                        }
                    }
                    Some(Command::RouteEnd) => match ROUTE_RX.lock(|r| r.borrow_mut().take()) {
                        Some(data) => {
                            info!("usb: route end, {} bytes", data.len());
                            ROUTE_RECEIVED.signal(Some(data));
                        }
                        None => warn!("usb: route end without begin"),
                    },
                    Some(Command::RouteClear) => ROUTE_RECEIVED.signal(None),
                    Some(Command::BenchMap) => BENCH_MAP.signal(()),
                    Some(Command::StorageFill(fill)) => STORAGE_FILL.signal(fill),
                    Some(Command::Light) => info!(
                        "light: theme {}, backlight {} %, sensor {:?}",
                        if NIGHT.load(Ordering::Relaxed) { "night" } else { "day" },
                        BACKLIGHT.load(Ordering::Relaxed),
                        STATUS.lock(Cell::get).light
                    ),
                    Some(Command::SimLight(raw)) => {
                        info!("usb: ambient light {raw:?}");
                        SIM_LIGHT.lock(|c| c.set(raw));
                    }
                    Some(Command::SimFix(Some(at))) => {
                        if !sim_fix_active() {
                            info!("usb: simulated position, the receiver's ignored");
                        }
                        SIM_FIX_AT.lock(|c| c.set(Some(Instant::now())));
                        STATUS.lock(|c| {
                            let mut s = c.get();
                            let fix = sim_fix(&s, at);
                            s.set_fix(fix, Instant::now().as_millis());
                            c.set(s);
                        });
                        STATUS_CHANGED.signal(());
                    }
                    Some(Command::SimFix(None)) => {
                        SIM_FIX_AT.lock(|c| c.set(None));
                        info!("usb: simulated position off");
                    }
                    Some(Command::RtcShift(s)) => match STATUS.lock(Cell::get).rtc {
                        Some(rtc) => {
                            let t = RtcTime::from_unix(rtc.unix() + s as i64);
                            info!("usb: rtc {rtc:?} -> {t:?} (shift {s} s)");
                            NRF_COMMANDS.send(command::set_rtc(&t, t.weekday())).await;
                        }
                        None => warn!("usb: rtc shift, no RTC time yet"),
                    },
                    Some(Command::RouteDataDamaged) => {}
                    None if !text.is_empty() => warn!("usb: unknown command {text:?}"),
                    None => {}
                }
                len = 0;
            } else if len < line.len() {
                line[len] = b;
                len += 1;
            }
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

/// A valid fix at this position, timed by the RTC (the recorder takes one point per new second), without altitude
/// (the altimeter keeps to the barometer).
pub(crate) fn sim_fix(s: &Status, at: test_hook::SimFix) -> gnss::Fix {
    let mut f = gnss::Fix::default();
    f.valid = true;
    f.quality = 1;
    f.sats_used = 9;
    f.sats_in_view = 9;
    f.hdop_c = Some(100);
    f.lat_e7 = Some(at.lat_e7);
    f.lon_e7 = Some(at.lon_e7);
    f.speed_mm_s = at.speed_mm_s;
    f.course_cdeg = at.course_cdeg;
    if let Some(t) = s.rtc {
        f.time = Some(gnss::Time { hour: t.hour, minute: t.minute, second: t.second, millis: 0 });
        f.date = Some(gnss::Date { year: t.year, month: t.month, day: t.day });
    }
    f
}
