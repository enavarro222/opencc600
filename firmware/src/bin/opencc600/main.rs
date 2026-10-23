//! Bring-up firmware (docs/PRODUCT.md, technical plan).
//!
//! Step 0: an esp-hal image in `ota_0` boots under the vendor bootloader, with the 2 MB quad PSRAM.
//! Panel: ST7789 on the i8080 16-bit bus, drawn with `embedded-graphics` (the test pattern only in the bench).

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::Cell;
use core::sync::atomic::Ordering;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::dma_tx_buffer;
use esp_hal::gpio::{DriveMode, Level, Output, OutputConfig};
use esp_hal::interrupt::Priority;
use esp_hal::lcd_cam::lcd::i8080::{self, I8080};
use esp_hal::lcd_cam::LcdCam;
use esp_hal::ledc::channel::{self, ChannelIFace};
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed};
use esp_hal::ledc::timer::{self, TimerIFace};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use esp_rtos::embassy::InterruptExecutor;
use esp_storage::FlashStorage;
use littlefs2::fs::{Allocation, Filesystem};
use log::{debug, info, warn};
use nrf_link::{KeyAction, command};
use opencc600_firmware::{ant, ble};
use opencc600_firmware::gnss_uart::GnssUart;
#[cfg(feature = "hil-storage")]
use opencc600_firmware::hil_storage;
use opencc600_firmware::panel::{self, FrameBuffer, Panel};
use opencc600_firmware::storage;
use protocol_cc600::transfer::Kind;
use ride_store::room;
use static_cell::StaticCell;
use status::Status;

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

mod app;
mod config;
mod nrf;
mod pairing;
mod receiver;
mod ride;
mod route_file;
mod screen;
mod trip_log;
mod usb;

use app::{App, RIDE_ANSWER, RIDE_REQUEST, RideAnswer, serve_rides};
use config::{SETTINGS, SETTINGS_CHANGED, follow_route, load_settings, save_settings, settings, utc_offset};
use nrf::{NRF_COMMANDS, NRF_KEYS, SOUND_ON, STREAM_AT, nrf_link};
use pairing::{LISTED_FOUND, ROLE_BITS, SCAN_ROLE, SHOWN_FOUND, calibrate_power, paired, role_index, set_sensor};
use receiver::gnss_task;
use ride::{Ride, StartError, StoredRide, ride_card, stored_rides};
use route_file::{MAP_RECEIVED, ROUTE_RECEIVED, bench_map, load_map, load_route, store_map, store_route};
use screen::{BACKLIGHT, NIGHT, bench, draw_message, draw_status, theme, view_of};
use trip_log::TripLog;
use usb::{BENCH_MAP, DRY_POWER_OFF, SCREEN, SHOWN_RIDE, SIM_LIGHT, STORAGE_FILL, storage_fill, storage_fill_start, usb_keys};

// App descriptor read by the ESP-IDF second-stage bootloader (the vendor one stays in place).
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    // 2 MB embedded quad PSRAM (AP_3v3); set explicitly, `Auto` probes octal first.
    // 80 MHz like the vendor (`CONFIG_SPIRAM_SPEED_80M`); the esp-hal default is 40 MHz.
    esp_alloc::psram_allocator!(
        peripherals.PSRAM,
        esp_hal::psram,
        esp_hal::psram::PsramConfig {
            mode: esp_hal::psram::PsramMode::QuadSpi,
            ram_frequency: esp_hal::psram::SpiRamFreq::Freq80m,
            ..Default::default()
        }
    );

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("opencc600: booted");
    info!("heap: {}", esp_alloc::HEAP.stats());

    // Panel before backlight, so the stale frame left in the GRAM is never shown.
    // RD is held high (we never read from the panel); `_rd` must live as long as the panel.
    let _rd = Output::new(peripherals.GPIO39, Level::High, OutputConfig::default());
    let lcd_cam = LcdCam::new(peripherals.LCD_CAM);
    let bus = I8080::new(
        lcd_cam.lcd,
        peripherals.DMA_CH0,
        i8080::Config::default().with_frequency(Rate::from_mhz(15)),
    )
    .unwrap()
    .with_cs(peripherals.GPIO2)
    .with_dc(peripherals.GPIO40)
    .with_wrx(peripherals.GPIO3)
    .with_data0(peripherals.GPIO4)
    .with_data1(peripherals.GPIO38)
    .with_data2(peripherals.GPIO5)
    .with_data3(peripherals.GPIO37)
    .with_data4(peripherals.GPIO6)
    .with_data5(peripherals.GPIO36)
    .with_data6(peripherals.GPIO7)
    .with_data7(peripherals.GPIO35)
    .with_data8(peripherals.GPIO8)
    .with_data9(peripherals.GPIO34)
    .with_data10(peripherals.GPIO9)
    .with_data11(peripherals.GPIO33)
    .with_data12(peripherals.GPIO10)
    .with_data13(peripherals.GPIO47)
    .with_data14(peripherals.GPIO11)
    .with_data15(peripherals.GPIO48);
    let dma_buf = dma_tx_buffer!(panel::BAND_BYTES).unwrap();
    let mut panel = Panel::new(bus, dma_buf);
    info!("panel initialised");

    // Which device we are: CC600 or CC700 from the vendor's factory data (only read), the Bluetooth name and address
    // from the MAC address.
    let mut flash = FlashStorage::new(peripherals.FLASH);
    let me = identify(&mut flash);
    info!("device: {:?}, Bluetooth name {}", me.model, me.name.as_str());

    // Storage: our littlefs partition, formatted here only if blank (tools/flash.py erases the vendor's).
    let mut partition = storage::Partition::new(flash);
    let mut alloc = Box::new(Allocation::new());
    let fs = match storage::open(&mut alloc, &mut partition) {
        Ok((fs, formatted)) => {
            if formatted {
                info!("storage: blank partition, formatted");
            }
            Ok(fs)
        }
        Err(e) => {
            warn!("storage: littlefs error {}", e.code());
            Err(e.code())
        }
    };
    // Rides interrupted by a power cut, a crash or a power off during a pause: the newest one is offered to resume
    // (reopened paused below), older ones are closed with what had been written.
    let interrupted = fs
        .as_ref()
        .ok()
        .and_then(|fs| recorder::interrupted(fs).ok().flatten());
    if let Ok(fs) = fs.as_ref()
        && let Err(e) = recorder::recover(fs, utc_offset(None), interrupted, |r| {
            info!("ride recovered: {r:?}")
        })
    {
        warn!("ride recovery: littlefs error {}", e.code());
    }
    let loaded = fs
        .as_ref()
        .map_or_else(|_| settings::Settings::default(), load_settings);
    SETTINGS.lock(|s| s.set(Some(loaded)));
    NIGHT.store(loaded.theme == settings::Theme::Night, Ordering::Relaxed);
    SOUND_ON.store(loaded.sound, Ordering::Relaxed);
    ble::PAIRED.signal(paired(&loaded));
    ant::pair(&paired(&loaded));
    let storage_info = fs.as_ref().map_err(|&code| code).and_then(storage_summary);
    STATUS.lock(|s| {
        let mut status = s.get();
        status.storage = Some(storage_info);
        s.set(status);
    });
    #[cfg(feature = "hil-storage")]
    let mut hil = fs.as_ref().ok().map(|fs| {
        let next = hil_storage::recover(fs).unwrap();
        hil_storage::Recorder::start(fs, next).unwrap()
    });

    // GNSS: UART0 through UHCI DMA, drained by a task on the thread-mode executor (the DMA does not need it to be
    // fast, only regular).
    let gnss = GnssUart::new(
        peripherals.UART0,
        peripherals.GPIO1,
        peripherals.GPIO0,
        peripherals.UHCI0,
        peripherals.DMA_CH1,
    );
    spawner.spawn(gnss_task(gnss).unwrap());
    // Test hook: key presses typed on the USB serial port (esp-println keeps writing the log on the TX side).
    let (usb_rx, _usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).split();
    spawner.spawn(usb_keys(usb_rx).unwrap());
    spawner.spawn(ble_task(peripherals.BT, me).unwrap());

    // the logo until the first screen (the backlight comes on next)
    let mut fb = FrameBuffer::new();
    let mut splash = ui::Ui::new();
    splash.set_identity(me.model.name(), me.name.as_str());
    splash.show_splash();
    let _ = ui::draw(&mut fb, &splash, &ui::View::default(), None, None, &theme());
    panel.flush(&fb);

    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);
    let mut bl_timer = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    bl_timer
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty10Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_khz(20),
        })
        .unwrap();
    // Backlight on GPIO45, LEDC low-speed timer 0 / channel 0, 10-bit, 20 kHz, active high (c606-oss
    // `main/backlight.c`, mirrors the vendor `MidLcdPwmInit()`); its level is the `backlight` setting.
    let mut bl = ledc.channel(channel::Number::Channel0, peripherals.GPIO45);
    bl.configure(channel::config::Config {
        timer: &bl_timer,
        duty_pct: settings().backlight,
        drive_mode: DriveMode::PushPull,
    })
    .unwrap();
    BACKLIGHT.store(settings().backlight, Ordering::Relaxed);
    info!("backlight on ({} %)", settings().backlight);

    if RUN_BENCH {
        bench(&mut panel, &mut fb);
    }

    // nRF link on an interrupt executor, so it preempts the main loop: the UART FIFO holds 128 bytes, ~11 ms at
    // 115200 baud, less than a screen redraw + flush (~25 ms) on the thread-mode executor.
    static NRF_EXECUTOR: StaticCell<InterruptExecutor<1>> = StaticCell::new();
    let nrf_spawner = NRF_EXECUTOR
        .init(InterruptExecutor::new(peripherals.FROM_CPU_INTR1))
        .start(Priority::Priority2);
    nrf_spawner.spawn(nrf_link(peripherals.UART2, peripherals.GPIO42, peripherals.GPIO41).unwrap());

    // Vendor boot (`INIT_QUERY`): power-on command, then the power-on reason query.
    NRF_COMMANDS.send(command::power_on()).await;
    NRF_COMMANDS.send(command::check_power_on_reason()).await;

    let mut shown = None;
    let mut scan_sent: Option<Instant> = None;
    let mut last_power_on = Instant::now();
    let mut ui = ui::Ui::new();
    ui.set_identity(me.model.name(), me.name.as_str());
    ui.set_scroll_s(settings().scroll_s);
    ui.set_auto_off_min(settings().auto_off_min);
    let mut ride: Option<Box<Ride>> = None;
    if let (Some(id), Ok(fs)) = (interrupted, fs.as_ref()) {
        ride = Ride::reopen(fs, id, false);
        if ride.is_some() {
            ui.found_paused_ride();
        }
    }
    ui.show_splash();
    // the newest finished ride: resumable until uploaded (no upload yet)
    let mut last_finished = fs
        .as_ref()
        .ok()
        .and_then(|fs| recorder::last_finished(fs).ok().flatten());
    // the ride in progress, else the last one saved (the "saved" screen)
    let mut ride_shown = ride.as_deref().map(Ride::info).unwrap_or_default();
    let mut trip_log: Option<TripLog> = None;
    // the finished rides while the rides screen is shown, and what it shows
    let mut stored: Option<Vec<StoredRide>> = None;
    let mut rides_view = ui::RidesView::default();
    // the ride's switch to a point every 5 s, told once
    let mut sparse_told = false;
    let mut auto_backlight = ui::light::AutoBacklight::new();
    // the last position known, for the time of sunset and sunrise
    let mut here: Option<(i32, i32)> = None;
    let mut effort_pick = ui::EffortPick::default();
    // the blocks the test hook's filler file leaves free (`storage fill`), while it is written
    let mut fill_left: Option<usize> = None;
    // the free space, for the hours of riding left on the home screen: read again now and then
    let mut storage_read = Instant::now();
    // the route (`/route`), its background map (`/map`) and where the rider is on it; read once the BLE controller has
    // its internal RAM (never blocking the boot for long)
    let _ = with_timeout(Duration::from_secs(3), ble::CONTROLLER_UP.wait()).await;
    let mut route_data = fs.as_ref().ok().and_then(load_route);
    let mut map_data = fs.as_ref().ok().and_then(|fs| load_map(fs, route_data.as_deref()));
    // why the last map received was not kept, for its banner
    let mut map_refused: Option<&'static str> = None;
    let mut nav = ui::nav::Nav::default();
    let mut route_shown: (Option<ui::RouteView>, bool) = (None, false);
    let mut auto_off: Option<ui::Command> = None;
    let mut auto_off_announced = false;
    let mut unplugged_at: Option<Instant> = None;
    loop {
        // the charging screen's animation: 20 frames a second (a redraw and flush take ~25 ms, nothing else runs)
        let animated = ui.screen() == ui::Screen::Charging;
        // the app's ride requests are served while waiting: an iteration (and its redraw) for each piece of a ride
        // made a download 3 times slower
        let wait = if animated { Duration::from_millis(25) } else { Duration::from_secs(1) };
        let sent = with_timeout(wait, async {
            loop {
                // the request first: the nRF and the GNSS signal a change several times a second
                match select(RIDE_REQUEST.wait(), STATUS_CHANGED.wait()).await {
                    Either::Second(()) => return false,
                    Either::First(r) => {
                        let answer = fs
                            .as_ref()
                            .map_err(|_| protocol_cc600::ErrorCode::Storage)
                            .and_then(|fs| serve_rides(fs, r));
                        let sent = matches!(answer, Ok(RideAnswer::Sent));
                        RIDE_ANSWER.signal(answer);
                        if sent {
                            return true;
                        }
                    }
                }
            }
        })
        .await
        .unwrap_or(false);
        if sent {
            // the rides screen shows it sent
            stored = None;
            ui.inform("Sortie envoyée", ui::fonts::Line::new(format_args!("À l'appli")), Instant::now().as_millis());
        }
        if let Some(new) = ROUTE_RECEIVED.try_take()
            && let Ok(fs) = fs.as_ref()
        {
            route_data = store_route(fs, new);
            // the stored map stays for the route it was made for (the app sends the new one right after); the one
            // shown goes first: two maps at once may not fit the PSRAM
            drop(map_data.take());
            map_data = load_map(fs, route_data.as_deref());
            // the room the app asks for next
            refresh_storage(fs);
            nav = ui::nav::Nav::default();
            // a route sent is the one to follow
            if route_data.is_some() && !settings().follow_route {
                follow_route(true);
            }
        }
        // after the route: a map comes right after its route
        if let Some(new) = MAP_RECEIVED.try_take()
            && let Ok(fs) = fs.as_ref()
        {
            // the map shown goes first, as above
            drop(map_data.take());
            map_data = match store_map(fs, new, route_data.as_deref()) {
                Ok(map) => Some(map),
                Err(why) => {
                    map_refused = Some(why);
                    load_map(fs, route_data.as_deref())
                }
            };
            refresh_storage(fs);
        }
        // what the companion app did, said on the screen
        if let Some(e) = ble::APP.try_take() {
            let now = Instant::now().as_millis();
            match e {
                companion::Event::Receiving { kind, received, size } => {
                    let what = if kind == Kind::Route { "Réception de la trace" } else { "Réception de la carte" };
                    let pct = u64::from(received) * 100 / u64::from(size.max(1));
                    ui.progress(what, ui::fonts::Line::new(format_args!("{pct} %")), received, size, now);
                }
                companion::Event::Received(Kind::Route) => {
                    let name = route_data
                        .as_deref()
                        .and_then(|d| route::Route::parse(d).ok())
                        .map_or_else(String::new, |r| String::from(r.name()));
                    ui.inform("Trace reçue", ui::fonts::Line::new(format_args!("{name}")), now);
                }
                companion::Event::Received(Kind::Map) => match map_refused.take() {
                    Some(why) => ui.inform("Carte refusée", ui::fonts::Line::new(format_args!("{why}")), now),
                    None => ui.inform("Carte reçue", ui::fonts::Line::new(format_args!("")), now),
                },
                companion::Event::SettingsChanged => {
                    ui.inform("Réglages reçus", ui::fonts::Line::new(format_args!("Depuis l'appli")), now)
                }
            }
        }
        if SETTINGS_CHANGED.signaled() {
            SETTINGS_CHANGED.reset();
            let s = settings();
            if let Ok(fs) = fs.as_ref() {
                save_settings(fs, &s);
            }
            SOUND_ON.store(s.sound, Ordering::Relaxed);
            ble::PAIRED.signal(paired(&s));
            ant::pair(&paired(&s));
            if s.auto_backlight {
                // set at the next light level
                auto_backlight = ui::light::AutoBacklight::new();
            } else {
                info!("backlight: {} % (setting)", s.backlight);
                let _ = bl.set_duty(s.backlight);
                BACKLIGHT.store(s.backlight, Ordering::Relaxed);
            }
            ui.set_scroll_s(s.scroll_s);
            ui.set_auto_off_min(s.auto_off_min);
        }
        // SendPowerOnCmd until the nRF answers, as the vendor: each one kills the ANT+ channels opened within ~6 s
        // at the nRF's next 5 s tick (c606-oss), so every 5 s no channel lived more than a few seconds.
        if STATUS.lock(Cell::get).power_on_reason.is_none() && last_power_on.elapsed() >= Duration::from_secs(5) {
            NRF_COMMANDS.send(command::power_on()).await;
            last_power_on = Instant::now();
        }
        if let Some(frame) = ant::poll() {
            NRF_COMMANDS.send(frame).await;
        }
        // What each key does is up to the screen (crates/ui): bottom right starts / pauses / resumes, a ride ends
        // from the pause screen (confirmed), power off is a long press on the top left key of the home screen or
        // the pause menu.
        let resumable = ride.is_none() && last_finished.is_some();
        let mut view = view_of(&STATUS.lock(Cell::get), &ride_shown, resumable);
        // the route as last shown: the menu offers to set it aside or follow it
        (view.route, view.route_set_aside) = route_shown;
        view.rides = rides_view;
        // the automatic power off asked by the last tick
        let mut commands: Vec<ui::Command> = auto_off.take().into_iter().collect();
        // the logo until the nRF's sensor stream shows up (at least 1.5 s), or is known missing; then, without it, until
        // the charger's state is known (the nRF sends it ~5 s after a cold start): straight to the charging screen
        // when plugged in, not through the home screen
        if ui.screen() == ui::Screen::Splash {
            let up_ms = Instant::now().as_millis();
            let stream = STREAM_AT.lock(Cell::get).is_some();
            let charger_known = STATUS.lock(Cell::get).charger.is_some();
            if (up_ms >= 1_500 && stream) || (up_ms >= 3_500 && charger_known) || up_ms >= 8_000 {
                info!("splash: done at {up_ms} ms, stream {stream}, charger {:?}", STATUS.lock(Cell::get).charger);
                ui.end_splash();
            }
        }
        // Like the vendor ("off, charging"): started by the charger, or after a power off with USB in, the nRF has no
        // sensor stream until its power-on gesture: the charging screen until a long press. Unplugged there: off.
        if view.stream_lost && view.plugged && matches!(ui.screen(), ui::Screen::Home | ui::Screen::Resume) {
            info!("charging screen: no sensor stream from the nRF, on USB power");
            ui.show_charging();
        }
        if ui.screen() == ui::Screen::Charging && !view.plugged {
            let since = *unplugged_at.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(10) {
                info!("charging screen: unplugged, switching off");
                commands.push(ui::Command::PowerOff);
                unplugged_at = None;
            }
        } else {
            unplugged_at = None;
        }
        while let Ok((key, action)) = NRF_KEYS.try_receive() {
            let press = match action {
                KeyAction::Click => ui::Press::Click,
                KeyAction::LongPress => ui::Press::Long,
                KeyAction::LongRelease | KeyAction::Other(_) => continue,
            };
            let Some(key) = ui::Key::from_nrf(key) else {
                continue;
            };
            if let Some(c) = ui.key(key, press, Instant::now().as_millis(), &view) {
                commands.push(c);
            }
        }
        let state = STATUS.lock(Cell::get);
        for c in commands {
            match (c, ride.as_mut(), fs.as_ref()) {
                (ui::Command::PowerOff, _, _) => {
                    // a ride is paused if needed and kept: offered again at power-on
                    if let (Some(r), Ok(fs)) = (ride.as_mut(), fs.as_ref()) {
                        if r.info().running {
                            r.toggle_pause(fs, &state);
                        }
                        r.keep(fs);
                    }
                    info!("power off");
                    draw_message(&mut fb, "Power off");
                    panel.flush(&fb);
                    if DRY_POWER_OFF.load(Ordering::Relaxed) {
                        // a real power off stops the nRF's sensor stream (time, barometer) until a long press
                        info!("power off: test hook, restarting instead");
                        Timer::after(Duration::from_millis(200)).await;
                        esp_hal::system::software_reset();
                    }
                    NRF_COMMANDS.send(command::power_off()).await;
                    Timer::after(Duration::from_secs(2)).await;
                    warn!("still powered 2 s after power_off");
                }
                (ui::Command::Start, None, Ok(fs)) => match Ride::start(fs, &state) {
                    Ok(r) => ride = Some(r),
                    Err(e) => {
                        let (title, detail) = match e {
                            StartError::NoTime => {
                                ("Pas encore l'heure", ui::fonts::Line::new(format_args!("Attends le GPS")))
                            }
                            // the rides screen, to delete some
                            StartError::Full => {
                                ui.make_room();
                                continue;
                            }
                            StartError::Storage(code) => {
                                ("Sortie pas démarrée", ui::fonts::Line::new(format_args!("Erreur mémoire {code}")))
                            }
                        };
                        ui.inform(title, detail, Instant::now().as_millis());
                    }
                },
                (ui::Command::Start, None, Err(&code)) => {
                    let detail = ui::fonts::Line::new(format_args!("Mémoire illisible ({code})"));
                    ui.inform("Sortie pas démarrée", detail, Instant::now().as_millis());
                }
                (ui::Command::ResumeLast, None, Ok(fs)) => {
                    if let Some(id) = last_finished.take() {
                        ride = Ride::reopen(fs, id, true);
                        if let Some(r) = ride.as_mut() {
                            r.toggle_pause(fs, &state);
                        }
                    }
                }
                (ui::Command::Pause, Some(r), Ok(fs)) if r.info().running => {
                    r.toggle_pause(fs, &state)
                }
                (ui::Command::Resume, Some(r), Ok(fs)) if !r.info().running => {
                    r.toggle_pause(fs, &state)
                }
                (ui::Command::Stop, Some(_), Ok(fs)) => {
                    let r = ride.take().unwrap();
                    ride_shown = r.info();
                    last_finished = Some(r.recorder.id());
                    // it takes a while (the file sealed, the storage read again): said on the screen first
                    let saving = ui::View { saving: true, ..view_of(&state, &ride_shown, false) };
                    ui::draw(&mut fb, &ui, &saving, None, None, &theme()).unwrap();
                    panel.flush(&fb);
                    let t = Instant::now();
                    r.stop(fs, &state);
                    refresh_storage(fs);
                    info!("ride: saved in {} ms", t.elapsed().as_millis());
                    // the keys pressed meanwhile did nothing on the screen: dropped, not acted on later
                    while NRF_KEYS.try_receive().is_ok() {}
                }
                // a ride started by mistake: its file goes, a resumed ride with what it had before
                (ui::Command::Discard, Some(_), Ok(fs)) => {
                    let id = ride.take().unwrap().recorder.id();
                    let now = Instant::now().as_millis();
                    match ride_store::remove_unfinished(fs, id) {
                        Ok(()) => {
                            info!("ride {id}: discarded");
                            ui.inform("Sortie effacée", ui::fonts::Line::new(format_args!("")), now);
                        }
                        // the file stays: offered again at the next power-on
                        Err(e) => {
                            warn!("ride {id}: discard failed, littlefs error {}", e.code());
                            let detail = ui::fonts::Line::new(format_args!("Erreur mémoire {}", e.code()));
                            ui.inform("Pas effacée", detail, now);
                        }
                    }
                    refresh_storage(fs);
                }
                // the scan follows the scan screen (below)
                (ui::Command::Scan(_) | ui::Command::StopScan, _, _) => {}
                (ui::Command::Pair(role, i), _, _) => {
                    if let Some(sensor) = SHOWN_FOUND.lock(Cell::get).get(i as usize).copied().flatten() {
                        set_sensor(role, sensor);
                    }
                }
                (ui::Command::Forget(role), _, _) => set_sensor(role, settings::Sensor::None),
                (ui::Command::Calibrate, _, _) => calibrate_power(),
                (ui::Command::WakeUp, _, _) => info!("charging screen: switched on"),
                (ui::Command::DeleteRide(i), None, Ok(fs)) => {
                    if let Some(r) = stored.take().and_then(|list| list.into_iter().nth(usize::from(i))) {
                        match ride_store::remove(fs, r.id) {
                            Ok(()) => {
                                info!("ride {}: deleted ({})", r.id, if r.sent { "sent" } else { "not sent" });
                                let detail = ui::fonts::Line::new(format_args!(""));
                                ui.inform("Sortie supprimée", detail, Instant::now().as_millis());
                            }
                            Err(e) => {
                                warn!("ride {}: delete failed, littlefs error {}", r.id, e.code());
                                let detail = ui::fonts::Line::new(format_args!("Erreur mémoire {}", e.code()));
                                ui.inform("Pas supprimée", detail, Instant::now().as_millis());
                            }
                        }
                        // the end screen's "resume" was maybe this one
                        last_finished = recorder::last_finished(fs).ok().flatten();
                        refresh_storage(fs);
                    }
                }
                (ui::Command::FollowRoute(follow), _, _) => {
                    follow_route(follow);
                    let now = Instant::now().as_millis();
                    if follow {
                        let name = route_data
                            .as_deref()
                            .and_then(|d| route::Route::parse(d).ok())
                            .map_or_else(String::new, |r| String::from(r.name()));
                        ui.inform("Trace suivie", ui::fonts::Line::new(format_args!("{name}")), now);
                    } else {
                        ui.inform("Parcours libre", ui::fonts::Line::new(format_args!("Trace gardée")), now);
                    }
                }
                (c, _, _) => warn!("ui: {c:?} ignored"),
            }
        }
        // a scan while the scan screen is shown (it drops the sensor links meanwhile), renewed before it ends
        if ui.screen() == ui::Screen::SensorScan {
            SCAN_ROLE.lock(|c| c.set(Some(ROLE_BITS[role_index(ui.role())])));
            if scan_sent.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(20)) {
                ble::SCAN.signal(30);
                NRF_COMMANDS.send(ant::search(30, scan_sent.is_none())).await;
                scan_sent = Some(Instant::now());
            }
        } else if scan_sent.take().is_some() {
            SCAN_ROLE.lock(|c| c.set(None));
            ble::SCAN.signal(0);
            NRF_COMMANDS.send(ant::search(0, false)).await;
        }
        if let Ok(fs) = fs.as_ref() {
            if let Some(r) = ride.as_mut() {
                r.sample(fs, &state);
            }
            // only when asked for (`set trip_log on`): it takes ~5 times what a ride does
            let wanted = settings().trip_log;
            if trip_log.is_none() && wanted {
                trip_log = TripLog::start(&state);
            } else if !wanted && let Some(mut log) = trip_log.take() {
                log.flush(fs);
            }
            if let Some(log) = trip_log.as_mut() {
                log.sample(fs, &state, ride.as_deref().map(Ride::info));
            }
            // the free room read again once filled or emptied
            let filled = if let Some(fill) = STORAGE_FILL.try_take() {
                fill_left = storage_fill_start(fs, fill);
                fill_left.is_none()
            } else if let Some(left) = fill_left {
                fill_left = storage_fill(fs, left);
                fill_left.is_none()
            } else {
                false
            };
            if filled {
                refresh_storage(fs);
            }
        }
        if let Ok(fs) = fs.as_ref()
            && storage_read.elapsed() >= Duration::from_secs(300)
        {
            storage_read = Instant::now();
            refresh_storage(fs);
        }
        let ride_info = ride.as_deref().map(Ride::info);
        if let Some(r) = ride_info {
            ride_shown = r;
        }
        let resumable = ride.is_none() && last_finished.is_some();
        // the rides screen's list: read when it opens, again after a deletion
        if matches!(ui.screen(), ui::Screen::Rides | ui::Screen::ConfirmDelete) {
            if stored.is_none()
                && let Ok(fs) = fs.as_ref()
            {
                stored = Some(stored_rides(fs, ui.making_room()));
            }
        } else {
            stored = None;
        }
        let list = stored.as_deref().unwrap_or_default();
        let selected = list.get(usize::from(ui.ride_item()));
        SHOWN_RIDE.store(selected.map_or(0, |r| r.id), Ordering::Relaxed);
        let room = STATUS.lock(Cell::get).storage.and_then(|r| r.ok()).map(|(_, free, _)| free);
        rides_view = ui::RidesView {
            count: list.len() as u16,
            card: selected.map(ride_card),
            can_start: room.is_some_and(|free| usize::from(free) * storage::BLOCK >= room::RIDE_START_BYTES),
        };
        let mut view = view_of(&state, &ride_shown, resumable);
        view.rides = rides_view;
        let route = route_data
            .as_deref()
            .and_then(|d| route::Route::parse(d).ok());
        // checked whole once, when it came: read at every frame, the CRC of a big map would take tens of ms
        let bg_map = map_data.as_deref().and_then(|d| route::map::Map::read(d).ok());
        if BENCH_MAP.try_take().is_some()
            && let Some(r) = route.as_ref()
        {
            bench_map(&mut fb, r, bg_map.as_ref(), &view);
            shown = None;
        }
        let fix = state.fix.filter(|f| f.valid).and_then(|f| {
            Some(ui::nav::Fix {
                lat_e7: f.lat_e7?,
                lon_e7: f.lon_e7?,
                speed_mm_s: f.speed_mm_s,
                course_cdeg: f.course_cdeg,
            })
        });
        // a route set aside (free ride): no route pages, no off-route alert; it stays stored
        let route = route.filter(|_| settings().follow_route);
        view.route = route.as_ref().map(|r| nav.update(r, fix));
        view.route_set_aside = route.is_none() && route_data.is_some();
        route_shown = (view.route, view.route_set_aside);
        let s = settings();
        if s.auto_backlight
            && let Some(raw) = SIM_LIGHT.lock(Cell::get).or(state.light[0])
            && let Some(pct) = auto_backlight.update(raw, Instant::now().as_millis())
        {
            info!("backlight: {pct} % (light {raw})");
            let _ = bl.set_duty(pct);
            BACKLIGHT.store(pct, Ordering::Relaxed);
        }
        if let Some(f) = fix {
            here = Some((f.lat_e7, f.lon_e7));
        }
        let night = match (s.auto_night, here, state.now_unix()) {
            (true, Some((lat, lon)), Some(unix)) => ui::light::is_night(unix.into(), lat, lon),
            _ => s.theme == settings::Theme::Night,
        };
        if NIGHT.swap(night, Ordering::Relaxed) != night {
            info!("theme: {}", if night { "night" } else { "day" });
        }
        // the Effort page with a power meter connected, else Cardio with a heart rate sensor
        let effort = effort_pick.update(ride_info.is_some(), &view.sensors, Instant::now().as_millis());
        // Climbs in place of Altitude when the route has climbs (a route of version 1 has none)
        let climbs = route.as_ref().is_some_and(|r| r.climbs().len() > 0);
        ui.set_context(route.is_some(), climbs, s.usage == settings::Usage::Train, effort);
        // the switch to a point every 5 s, said once a ride (the top bar keeps a mark)
        match ride_info {
            Some(r) if r.sparse && !sparse_told => {
                let detail = ui::fonts::Line::new(format_args!("1 point toutes les 5 s"));
                ui.inform("Mémoire presque pleine", detail, Instant::now().as_millis());
                sparse_told = true;
            }
            None => sparse_told = false,
            _ => {}
        }
        auto_off = ui.tick(
            Instant::now().as_millis(),
            ride_info.map(|r| r.running),
            &view,
        );
        if auto_off.is_some() {
            info!("auto off: still for {} min", settings().auto_off_min);
        }
        let announced = ui.banner().is_some_and(|b| b.title == ui::AUTO_OFF);
        if announced != auto_off_announced {
            auto_off_announced = announced;
            info!("auto off: {}", if announced { "announced" } else { "not announced" });
        }
        // the status screen shows (nearly) all of the status, the others only what is in the view
        let status_shown = (ui.screen() == ui::Screen::Status).then_some(state);
        let screen = (ui, view, status_shown, night);
        if shown != Some(screen) {
            let t = Instant::now();
            ui::draw(&mut fb, &ui, &view, route.as_ref(), bg_map.as_ref(), &theme()).unwrap();
            let ms = t.elapsed().as_millis();
            if ms > 100 {
                warn!("ui: {:?} {:?} drawn in {ms} ms", ui.screen(), ui.page());
            } else {
                debug!("ui: drawn in {ms} ms");
            }
            if status_shown.is_some() {
                draw_status(&mut fb, &state, ride_info);
            }
            panel.flush(&fb);
            shown = Some(screen);
            SHOWN_FOUND.lock(|c| c.set(LISTED_FOUND.lock(Cell::get)));
            let (items, sel) = ui.menu(&view);
            let item = items.get(sel as usize).copied().flatten();
            SCREEN.lock(|s| s.set((ui.screen(), item, ui.role(), ui.scan_item(), ui.page())));
        }
        // HIL: append back to back for 150 ms, so most resets land during a flash operation.
        #[cfg(feature = "hil-storage")]
        if let (Ok(fs), Some(recorder)) = (fs.as_ref(), hil.as_mut()) {
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(150) {
                if let Err(e) = recorder.step(fs) {
                    warn!("hil: append error {}", e.code());
                    break;
                }
            }
        }
        // At most 5 redraws per second (20 while animated); the nRF task keeps updating STATUS meanwhile, nothing is lost.
        Timer::after(Duration::from_millis(if animated { 25 } else { 200 })).await;
    }
}

const RUN_BENCH: bool = false;

/// Latest state from the nRF, written by the nRF task, read by the display loop.
static STATUS: Mutex<CriticalSectionRawMutex, Cell<Status>> = Mutex::new(Cell::new(Status::new()));

static STATUS_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "the BLE future (~15 KB, the GATT clients' notification queues) built once on the 215 KB main stack, \
    then kept on the heap"
)]
async fn ble_task(bt: esp_hal::peripherals::BT<'static>, me: &'static identity::Identity) {
    Box::pin(ble::run(bt, me, App::default())).await;
}

/// Reads the storage again for the screens (the rides, the hours of riding left).
fn refresh_storage(fs: &Fs<'_>) {
    let storage_info = storage_summary(fs);
    STATUS.lock(|s| {
        let mut status = s.get();
        status.storage = Some(storage_info);
        s.set(status);
    });
}

/// Finished rides, blocks free for the rides (the logs' included: they give way to rides), total blocks.
fn storage_summary(fs: &Filesystem<'_, storage::Partition<'_>>) -> Result<(u16, u16, u16), i32> {
    let t = Instant::now();
    let mut rides = 0u16;
    ride_store::finished(fs, |_, _| rides += 1).map_err(|e| e.code())?;
    let free = fs.available_blocks().map_err(|e| e.code())? as u16;
    app::FLASH_FREE_BLOCKS.store(u32::from(free), Ordering::Relaxed);
    let logs = room::logs_bytes(fs).map_err(|e| e.code())?.div_ceil(storage::BLOCK) as u16;
    let total = fs.total_blocks() as u16;
    info!(
        "storage: {rides} rides, {free} / {total} blocks free, logs {logs} blocks, read in {} ms",
        t.elapsed().as_millis()
    );
    Ok((rides, free + logs, total))
}

type Fs<'a> = Filesystem<'a, storage::Partition<'a>>;

/// The model from the `nvs` partition, the name and BLE address from the MAC address; a CC600 when the partition
/// cannot be read.
fn identify(flash: &mut FlashStorage<'_>) -> &'static identity::Identity {
    static ME: StaticCell<identity::Identity> = StaticCell::new();
    let mut nvs = alloc::vec![0; identity::NVS_SIZE];
    if let Err(e) = flash.read(identity::NVS_OFFSET, &mut nvs) {
        warn!("nvs: not read ({e:?})");
        nvs.clear();
    }
    ME.init(identity::Identity::new(&nvs, esp_hal::efuse::base_mac_address().as_bytes().try_into().unwrap_or([0; 6])))
}

/// `file_id.serial_number` of the FIT files: the last 4 bytes of the MAC address, unique to the device.
fn serial() -> u32 {
    let mac = esp_hal::efuse::base_mac_address();
    let b = mac.as_bytes();
    u32::from_be_bytes([b[2], b[3], b[4], b[5]])
}
