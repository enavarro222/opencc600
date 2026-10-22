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

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_futures::select::{Either, select};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_10X20};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::{Alignment, Text};
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::dma_tx_buffer;
use esp_hal::gpio::{DriveMode, Level, Output, OutputConfig};
use esp_hal::interrupt::Priority;
use esp_hal::lcd_cam::LcdCam;
use esp_hal::lcd_cam::lcd::i8080::{self, I8080};
use esp_hal::ledc::channel::{self, ChannelIFace};
use esp_hal::ledc::timer::{self, TimerIFace};
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed};
use esp_hal::peripherals::{GPIO41, GPIO42, UART2};
use esp_hal::time::{self as hal_time, Rate};
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{self, Uart, UartRx, UartTx};
use esp_hal::usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx};
use esp_rtos::embassy::InterruptExecutor;
use esp_storage::FlashStorage;
use littlefs2::fs::{Allocation, Filesystem};
use littlefs2::io::Error;
use log::{debug, info, warn};
use nrf_link::{Decoder, KeyAction, Message, RtcTime, command};
use opencc600_firmware::{ant, ble};
use protocol_cc600::transfer::Kind;
use opencc600_firmware::gnss_uart::{self, GnssUart};
#[cfg(feature = "hil-storage")]
use opencc600_firmware::hil_storage;
use opencc600_firmware::panel::{self, FrameBuffer, Panel};
use opencc600_firmware::storage;
use ride_store::room;
use static_cell::StaticCell;
use status::{GnssLink, Status};
use test_hook::Command;

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

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
    // the route (`/route`) and where the rider is on it
    let mut route_data = fs.as_ref().ok().and_then(load_route);
    let mut bg_map = map_for(route_data.as_deref());
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
            bg_map = map_for(route_data.as_deref());
            nav = ui::nav::Nav::default();
            // a route sent is the one to follow
            if route_data.is_some() && !settings().follow_route {
                follow_route(true);
            }
        }
        // what the companion app did, said on the screen
        if let Some(e) = ble::APP.try_take() {
            let now = Instant::now().as_millis();
            match e {
                companion::Event::Receiving { kind, received, size } => {
                    let what = if kind == Kind::Route { "Réception de la trace" } else { "Réception de la carte" };
                    let pct = u64::from(received) * 100 / u64::from(size.max(1));
                    ui.inform(what, ui::fonts::Line::new(format_args!("{pct} %")), now);
                }
                companion::Event::Received(Kind::Route) => {
                    let name = route_data
                        .as_deref()
                        .and_then(|d| route::Route::parse(d).ok())
                        .map_or_else(String::new, |r| String::from(r.name()));
                    ui.inform("Trace reçue", ui::fonts::Line::new(format_args!("{name}")), now);
                }
                companion::Event::Received(Kind::Map) => {
                    ui.inform("Carte reçue", ui::fonts::Line::new(format_args!("")), now)
                }
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
                    let storage_info = storage_summary(fs);
                    STATUS.lock(|s| {
                        let mut status = s.get();
                        status.storage = Some(storage_info);
                        s.set(status);
                    });
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
                    let storage_info = storage_summary(fs);
                    STATUS.lock(|s| {
                        let mut status = s.get();
                        status.storage = Some(storage_info);
                        s.set(status);
                    });
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
                        let storage_info = storage_summary(fs);
                        STATUS.lock(|s| {
                            let mut status = s.get();
                            status.storage = Some(storage_info);
                            s.set(status);
                        });
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
                let storage_info = storage_summary(fs);
                STATUS.lock(|s| {
                    let mut status = s.get();
                    status.storage = Some(storage_info);
                    s.set(status);
                });
            }
        }
        if let Ok(fs) = fs.as_ref()
            && storage_read.elapsed() >= Duration::from_secs(300)
        {
            storage_read = Instant::now();
            let storage_info = storage_summary(fs);
            STATUS.lock(|s| {
                let mut status = s.get();
                status.storage = Some(storage_info);
                s.set(status);
            });
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
/// When the last position came over USB (`fix`, tools/replay_gpx.py): until [`SIM_FIX_HOLD`] after it, the
/// receiver's epochs are ignored.
static SIM_FIX_AT: Mutex<CriticalSectionRawMutex, Cell<Option<Instant>>> = Mutex::new(Cell::new(None));
const SIM_FIX_HOLD: Duration = Duration::from_secs(5);
/// An ambient light level from USB (`light <raw>`), in place of the sensor's for the automatic backlight, until
/// `light off` or a reset.
static SIM_LIGHT: Mutex<CriticalSectionRawMutex, Cell<Option<u16>>> = Mutex::new(Cell::new(None));

fn sim_fix_active() -> bool {
    SIM_FIX_AT.lock(Cell::get).is_some_and(|t| t.elapsed() < SIM_FIX_HOLD)
}
static NRF_KEYS: Channel<CriticalSectionRawMutex, (u8, KeyAction), 8> = Channel::new();
/// The last barometer frame of the nRF's sensor stream: it only starts on the nRF's own power-on gesture
/// (a long press of the power key), not when the charger starts the device, nor after a power off.
static STREAM_AT: Mutex<CriticalSectionRawMutex, Cell<Option<Instant>>> = Mutex::new(Cell::new(None));

/// The nRF's sensor stream is missing (none for 3 s, past the first 3 s after boot).
fn stream_lost() -> bool {
    let quiet = Duration::from_secs(3);
    Instant::now().duration_since(Instant::from_ticks(0)) >= quiet
        && STREAM_AT.lock(Cell::get).is_none_or(|at| at.elapsed() >= quiet)
}
/// The key events since the last trip log line (its `keys` column), oldest first: key, then `C` click, `L` long
/// press (repeated while held), `R` its release, `?` other; `u` before the ones typed on the USB test hook.
static KEY_LOG: Mutex<CriticalSectionRawMutex, RefCell<heapless::String<64>>> =
    Mutex::new(RefCell::new(heapless::String::new()));

fn log_key(key: u8, action: KeyAction, usb: bool) {
    let a = match action {
        KeyAction::Click => 'C',
        KeyAction::LongPress => 'L',
        KeyAction::LongRelease => 'R',
        KeyAction::Other(_) => '?',
    };
    let event = format!("{}{key}{a}", if usb { "u" } else { "" });
    KEY_LOG.lock(|l| {
        let mut l = l.borrow_mut();
        // a long press repeats while held: once is enough
        if l.rsplit(';').next() == Some(event.as_str()) && a == 'L' {
            return;
        }
        // full (a stuck key): the oldest go
        while !l.is_empty() && l.len() + 1 + event.len() > l.capacity() {
            let rest = l.split_once(';').map_or("", |(_, r)| r);
            *l = heapless::String::try_from(rest).unwrap_or_default();
        }
        if !l.is_empty() {
            let _ = l.push(';');
        }
        let _ = l.push_str(&event);
    });
}

static NRF_COMMANDS: Channel<CriticalSectionRawMutex, [u8; command::CTRL_FRAME], 4> =
    Channel::new();

/// Requests to the GNSS task from the USB test hook (time-to-first-fix measurements).
enum GnssRequest {
    Start(gnss::Start),
    /// Answer the receiver's time requests (and each restart) with the RTC time (`$PAIR590`).
    TimeAiding(bool),
}
static GNSS_REQUESTS: Channel<CriticalSectionRawMutex, GnssRequest, 4> = Channel::new();

/// UART2, TX GPIO42 / RX GPIO41, 115200 8N1. Built here: the async UART halves are not `Send`, so they cannot be
/// handed over from the thread-mode executor.
#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: decoder + buffers, statically allocated by embassy"
)]
async fn nrf_link(uart: UART2<'static>, tx_pin: GPIO42<'static>, rx_pin: GPIO41<'static>) {
    let uart = Uart::new(uart, uart::Config::default().with_baudrate(115_200))
        .unwrap()
        .with_tx(tx_pin)
        .with_rx(rx_pin)
        .into_async();
    let (rx, tx) = uart.split();
    // SAFETY: awaited from a task, i.e. with the Context of the Embassy interrupt executor running it.
    let spawner = unsafe { Spawner::for_current_executor() }.await;
    spawner.spawn(nrf_tx(tx).unwrap());
    nrf_rx(rx).await
}

#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "the BLE future (~15 KB, the GATT clients' notification queues) built once on the 215 KB main stack, \
    then kept on the heap"
)]
async fn ble_task(bt: esp_hal::peripherals::BT<'static>, me: &'static identity::Identity) {
    Box::pin(ble::run(bt, me, App::default())).await;
}

#[embassy_executor::task]
async fn nrf_tx(mut tx: UartTx<'static, Async>) {
    loop {
        let frame = NRF_COMMANDS.receive().await;
        if let Err(e) = tx.write_async(&frame).await {
            warn!("nrf uart tx: {e:?}");
        }
    }
}

async fn nrf_rx(mut rx: UartRx<'static, Async>) -> ! {
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 64];
    loop {
        let n = match rx.read_async(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                warn!("nrf uart: {e:?}");
                continue;
            }
        };
        decoder.feed(&buf[..n], |frame| {
            let msg = Message::parse(&frame);
            ant::on_message(&msg);
            match msg {
                Message::Unknown => info!("nrf ? {frame:?}"),
                // the ANT+ commands' (the others come every 5 s)
                Message::Ack if frame.cmd() == 0x01 => info!("nrf ack {frame:?}"),
                // periodic: every 5 s (RTC, and the replies to the power-on command)
                Message::Battery { .. }
                | Message::Status(_)
                | Message::Rtc(_)
                | Message::Ack
                | Message::PowerOnReason(_)
                | Message::Version { .. } => debug!("nrf {msg:?}"),
                Message::Accel(_)
                | Message::Gyro(_)
                | Message::Baro { .. }
                | Message::Light { .. }
                | Message::Stream { .. } => {
                    debug!("nrf {msg:?}")
                }
                // logged by the ANT module
                Message::AntChannel { .. } | Message::AntFound { .. } | Message::AntPage { .. } => {}
                _ => info!("nrf {msg:?}"),
            }
            // the barometer only: on a charger start the nRF still sends one clock frame
            if matches!(msg, Message::Baro { .. }) {
                STREAM_AT.lock(|c| c.set(Some(Instant::now())));
            }
            if let Message::Key { key, action } = msg {
                log_key(key, action, false);
                let _ = NRF_KEYS.try_send((key, action));
            }
            STATUS.lock(|s| {
                let mut status = s.get();
                status.update(msg, Instant::now().as_millis());
                s.set(status);
            });
            STATUS_CHANGED.signal(());
        });
    }
}

#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: line buffer + fix, statically allocated by embassy"
)]
async fn gnss_task(mut uart: GnssUart) {
    let mut reader = gnss::LineReader::new();
    let mut fix = gnss::Fix::default();
    let mut last_report = Instant::now();
    // `is_done()` of the UHCI transfer goes up at every idle EOF: silence is the only sign of a stopped DMA (ring
    // full) or a silent receiver.
    let mut silence = gnss::Silence::new(Instant::now().as_millis());
    let mut last_rtc_set: Option<Instant> = None;
    // Output setup: sent once the receiver talks, again when a sentence it turns off shows up (power cycle).
    let mut setup_needed = true;
    let mut last_setup: Option<Instant> = None;
    let mut link = GnssLink::new();
    // Last (re)start of the receiver, for the time to first fix: ESP32 boot, restart seen, or commanded.
    let mut started = (Instant::now(), "boot");
    // Answer the receiver's time requests with the RTC time (`$PAIR590`): after a power cycle it starts without
    // the time. `gnss time off` turns it off for the TTFF measurements.
    let mut answer_time = true;
    let mut time_aid_at: Option<Instant> = None;
    loop {
        while let Ok(request) = GNSS_REQUESTS.try_receive() {
            match request {
                GnssRequest::Start(kind) => {
                    info!("gnss: {kind:?} start ({})", kind.command());
                    send_pair(&mut uart, kind.command());
                    started = (Instant::now(), kind.name());
                    link.ttff_ms = None;
                    if answer_time {
                        time_aid_at = Some(Instant::now() + Duration::from_secs(1));
                    }
                }
                GnssRequest::TimeAiding(on) => {
                    info!("gnss: time aiding {}", if on { "on" } else { "off" });
                    answer_time = on;
                }
            }
        }
        let mut epoch = false;
        let n = uart.drain(|data| {
            reader.feed(data, |line| {
                debug!("gnss {line}");
                epoch |= fix.update(line) == gnss::Epoch::Complete;
                if gnss::is_turned_off(line) {
                    setup_needed = true;
                }
                if let Some((cmd, result)) = gnss::pair_ack(line) {
                    info!("gnss: PAIR{cmd:03} -> {result}");
                }
                if let Some(aiding) = gnss::aiding_request(line) {
                    info!("gnss: aiding request {aiding:?}");
                    link.aiding_requests += 1;
                    if aiding == gnss::Aiding::Time && answer_time {
                        time_aid_at = Some(Instant::now());
                    }
                }
                if gnss::is_started(line) {
                    info!("gnss: receiver started");
                    link.restarts += 1;
                    // a commanded start also shows up here: keep its time
                    if started.0.elapsed() >= Duration::from_secs(5) {
                        started = (Instant::now(), "receiver start");
                        link.ttff_ms = None;
                    }
                    if answer_time {
                        time_aid_at = Some(Instant::now() + Duration::from_millis(500));
                    }
                }
            })
        });
        if n > 0
            && setup_needed
            && last_setup.is_none_or(|at| at.elapsed() >= Duration::from_secs(10))
        {
            info!("gnss: sending the output setup");
            for body in gnss::OUTPUT_SETUP {
                send_pair(&mut uart, body);
            }
            setup_needed = false;
            last_setup = Some(Instant::now());
            link.setups += 1;
        }
        if time_aid_at.is_some_and(|at| Instant::now() >= at) {
            time_aid_at = None;
            match STATUS.lock(Cell::get).rtc {
                Some(t) => {
                    let body = format!(
                        "PAIR590,{},{:02},{:02},{:02},{:02},{:02}",
                        t.year, t.month, t.day, t.hour, t.minute, t.second
                    );
                    info!("gnss: time aiding from the RTC: {body}");
                    send_pair(&mut uart, &body);
                    link.time_aids += 1;
                }
                None => warn!("gnss: no RTC time for the time aiding"),
            }
        }
        let now_ms = Instant::now().as_millis();
        if silence.poll(n, now_ms) {
            link.silences = silence.count;
            warn!(
                "gnss: no data for {} s (receiver off, or DMA stopped), powering it on",
                silence.silent_ms(now_ms) / 1000
            );
            if GNSS_POWER_LOST.load(Ordering::Relaxed) > 0 {
                GNSS_POWER_LOST.fetch_sub(1, Ordering::Relaxed);
                warn!("gnss: power-on dropped by the test hook");
            } else {
                NRF_COMMANDS.send(command::gps_power(1)).await;
            }
        }
        if epoch
            && fix.valid
            && link.ttff_ms.is_none()
            // epochs computed before a commanded restart can still be in flight
            && started.0.elapsed() >= Duration::from_millis(1500)
        {
            let ms = started.0.elapsed().as_millis() as u32;
            link.ttff_ms = Some(ms);
            info!(
                "gnss: first fix {}.{} s after {}, {}/{} sats",
                ms / 1000,
                ms % 1000 / 100,
                started.1,
                fix.sats_used,
                fix.sats_in_view
            );
        }
        if n > 0 || STATUS.lock(Cell::get).gnss_link != link {
            STATUS.lock(|s| {
                let mut status = s.get();
                status.gnss = Some(reader.stats);
                status.gnss_link = link;
                s.set(status);
            });
        }
        if epoch && !sim_fix_active() {
            let calibration = STATUS.lock(|s| {
                let mut status = s.get();
                status.set_fix(fix, Instant::now().as_millis());
                let calibration = calibrate(&mut status.altimeter, &fix);
                s.set(status);
                calibration
            });
            match calibration {
                Some(altitude::Gnss::Reset) => {
                    warn!("altitude: the GNSS disagreed for a minute, reset on it")
                }
                Some(altitude::Gnss::Rejected) => debug!("altitude: GNSS altitude rejected"),
                _ => {}
            }
            STATUS_CHANGED.signal(());
            sync_rtc(&fix, &mut last_rtc_set).await;
        }
        let expired = STATUS.lock(|s| {
            let mut status = s.get();
            let expired = status.expire_fix(Instant::now().as_millis());
            if expired {
                s.set(status);
            }
            expired
        });
        if expired {
            warn!("gnss: no epoch for {} s, the last fix is no longer valid", status::FIX_MAX_AGE_MS / 1000);
            STATUS_CHANGED.signal(());
        }
        if last_report.elapsed() >= Duration::from_secs(10) {
            last_report = Instant::now();
            info!(
                "gnss: {:?}, {:?}, fix {} q{} {}/{} sats{}",
                reader.stats,
                link,
                fix.valid,
                fix.quality,
                fix.sats_used,
                fix.sats_in_view,
                if silence.silent_ms(Instant::now().as_millis()) >= gnss::Silence::AFTER_MS {
                    " SILENT"
                } else {
                    ""
                }
            );
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

fn send_pair(uart: &mut GnssUart, body: &str) {
    let mut out = [0; gnss_uart::MAX_COMMAND];
    match gnss::write_command(body, &mut out) {
        Some(len) => uart.send(&out[..len]),
        None => warn!("gnss: command too long: {body}"),
    }
}

/// Calibrates the barometer on the GNSS altitude of a valid fix good enough for it.
fn calibrate(altimeter: &mut altitude::Altimeter, fix: &gnss::Fix) -> Option<altitude::Gnss> {
    let (true, Some(alt_dm), Some(hdop_c)) = (fix.valid, fix.alt_dm, fix.hdop_c) else {
        return None;
    };
    let sigma = altitude::gnss_sigma_m(hdop_c as f64 / 100.0, fix.sats_used)?;
    Some(altimeter.gnss(alt_dm as f64 / 10.0, sigma))
}

/// Writes the GNSS time to the nRF RTC when they differ by 2 s or more (the RTC frame has a 1 s resolution and
/// arrives at 5 Hz, so 1 s is jitter), at most once a minute in case a write does not take.
async fn sync_rtc(fix: &gnss::Fix, last_set: &mut Option<Instant>) {
    let (true, Some(time), Some(date)) = (fix.valid, fix.time, fix.date) else {
        return;
    };
    if last_set.is_some_and(|at| at.elapsed() < Duration::from_secs(60)) {
        return;
    }
    let gnss = RtcTime {
        year: date.year,
        month: date.month,
        day: date.day,
        hour: time.hour,
        minute: time.minute,
        second: time.second,
    };
    let rtc = STATUS.lock(Cell::get).rtc;
    if rtc.is_some_and(|rtc| (rtc.unix() - gnss.unix()).abs() < 2) {
        return;
    }
    info!("rtc: {rtc:?} -> {gnss:?} (GNSS)");
    NRF_COMMANDS
        .send(command::set_rtc(&gnss, gnss.weekday()))
        .await;
    *last_set = Some(Instant::now());
}

/// Finished rides, blocks free for the rides (the logs' included: they give way to rides), total blocks.
fn storage_summary(fs: &Filesystem<'_, storage::Partition<'_>>) -> Result<(u16, u16, u16), i32> {
    let t = Instant::now();
    let mut rides = 0u16;
    ride_store::finished(fs, |_, _| rides += 1).map_err(|e| e.code())?;
    let free = fs.available_blocks().map_err(|e| e.code())? as u16;
    let logs = room::logs_bytes(fs).map_err(|e| e.code())?.div_ceil(storage::BLOCK) as u16;
    let total = fs.total_blocks() as u16;
    info!(
        "storage: {rides} rides, {free} / {total} blocks free, logs {logs} blocks, read in {} ms",
        t.elapsed().as_millis()
    );
    Ok((rides, free + logs, total))
}

/// Offset from UTC of the local time (the `tz` setting, summer time included) at Unix time `unix` (now when
/// unknown: the rule's offset at 1970, standard time).
fn utc_offset(unix: Option<u32>) -> i32 {
    settings().tz.offset_s(unix.unwrap_or(0) as i64)
}

/// The settings (`/settings`), loaded at boot; changed by the test hook (`set key value`) until the app does it.
static SETTINGS: Mutex<CriticalSectionRawMutex, Cell<Option<settings::Settings>>> =
    Mutex::new(Cell::new(None));
/// A setting changed: to save and apply.
static SETTINGS_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
const SETTINGS_FILE: &str = "/settings";

fn settings() -> settings::Settings {
    SETTINGS.lock(Cell::get).unwrap_or_default()
}

/// The role whose sensors are being looked for (the scan screen), as a `ble::ROLE_*` bit.
static SCAN_ROLE: Mutex<CriticalSectionRawMutex, Cell<Option<u8>>> = Mutex::new(Cell::new(None));
/// The sensors the scan screen shows, in its order: what [`ui::Command::Pair`] picks from. Set when the screen is
/// drawn: [`sensors_view`] sorts the list again each time, by signal strength, and the order changes often.
static SHOWN_FOUND: Mutex<CriticalSectionRawMutex, Cell<[Option<settings::Sensor>; ui::FOUND]>> =
    Mutex::new(Cell::new([None; ui::FOUND]));
/// The list of the last [`sensors_view`], for [`SHOWN_FOUND`] once drawn.
static LISTED_FOUND: Mutex<CriticalSectionRawMutex, Cell<[Option<settings::Sensor>; ui::FOUND]>> =
    Mutex::new(Cell::new([None; ui::FOUND]));

const ROLE_BITS: [u8; 3] = [ble::ROLE_HR, ble::ROLE_SPEED_CADENCE, ble::ROLE_POWER];

fn role_index(role: ui::Role) -> usize {
    ui::Role::ALL.iter().position(|&r| r == role).unwrap_or(0)
}

/// The paired sensor of each role, in [`ui::Role::ALL`] order.
fn role_sensors(s: &settings::Settings) -> [settings::Sensor; 3] {
    [s.hr, s.speed_cadence, s.power]
}

/// Pairs a role with a sensor (or none): saved, and the BLE task told by the main loop.
fn set_sensor(role: ui::Role, sensor: settings::Sensor) {
    let mut s = settings();
    match role {
        ui::Role::HeartRate => s.hr = sensor,
        ui::Role::SpeedCadence => s.speed_cadence = sensor,
        ui::Role::Power => s.power = sensor,
    }
    info!("sensors: {role:?} = {sensor}");
    SETTINGS.lock(|c| c.set(Some(s)));
    SETTINGS_CHANGED.signal(());
}

/// What names a sensor on the screens, after its radio (a dual strap is listed twice): the name it gave, else the
/// end of its address or its ANT number.
fn sensor_name(sensor: &settings::Sensor) -> ui::fonts::Line {
    use settings::Sensor;
    match (ble::name_of(sensor), sensor) {
        (Some(n), _) => ui::fonts::Line::new(format_args!("BT {}", n.as_str())),
        (None, Sensor::Ble { addr, .. }) => ui::fonts::Line::new(format_args!("BT …{:02X}{:02X}", addr[1], addr[0])),
        (None, Sensor::Ant { device_type, number, .. }) => {
            let kind = match *device_type {
                sensors::ant::SPEED_CADENCE => " V+C",
                sensors::ant::CADENCE => " cadence",
                sensors::ant::SPEED => " vitesse",
                _ => "",
            };
            ui::fonts::Line::new(format_args!("ANT+ {number}{kind}"))
        }
        (None, Sensor::None) => ui::fonts::Line::new(format_args!("–")),
    }
}

/// The sensors screens' content: each role's sensor and link, and what the scan found for the role looked for.
#[allow(
    clippy::large_stack_frames,
    reason = "the names (64-byte lines) and the scan list, from the main loop (215 KB stack)"
)]
fn sensors_view() -> ui::Sensors {
    let paired = role_sensors(&settings());
    let mut out = ui::Sensors::default();
    for (row, sensor) in out.roles.iter_mut().zip(paired.iter()) {
        if *sensor != settings::Sensor::None {
            *row = ui::SensorRow {
                link: if ble::connected(sensor) || ant::connected(sensor) {
                    ui::Link::Connected
                } else {
                    ui::Link::Searching
                },
                name: Some(sensor_name(sensor)),
            };
        }
    }
    let mut shown = [None; ui::FOUND];
    if let Some(bit) = SCAN_ROLE.lock(Cell::get) {
        let role = ROLE_BITS.iter().position(|&b| b == bit).unwrap_or(0);
        // BLE and ANT+ together, closest first
        let mut all: heapless::Vec<(settings::Sensor, i8), { 2 * ui::FOUND }> =
            ble::found::<{ ui::FOUND }>(bit).into_iter().map(|h| (h.sensor, h.rssi_dbm)).collect();
        all.extend(ant::found::<{ ui::FOUND }>(role));
        all.sort_unstable_by_key(|&(_, rssi)| core::cmp::Reverse(rssi));
        for (i, (sensor, rssi_dbm)) in all.into_iter().take(ui::FOUND).enumerate() {
            shown[i] = Some(sensor);
            out.found[i] = Some(ui::Found {
                name: sensor_name(&sensor),
                rssi_dbm,
                paired: sensor == paired[role],
            });
        }
    }
    LISTED_FOUND.lock(|c| c.set(shown));
    out.calibration = power_calibration();
    out
}

/// The power meter's calibration, and when it was asked.
static CALIBRATION: Mutex<CriticalSectionRawMutex, Cell<(ui::Calibration, Instant)>> =
    Mutex::new(Cell::new((ui::Calibration::None, Instant::from_ticks(0))));
/// A calibration without an answer after this long failed (the meters answer within a few seconds).
const CALIBRATION_WAIT: Duration = Duration::from_secs(20);

/// Asks the paired power meter, over its radio, for a zero offset calibration.
fn calibrate_power() {
    let sensor = settings().power;
    let state = match sensor {
        settings::Sensor::Ble { .. } if ble::calibrate() => ui::Calibration::Running,
        settings::Sensor::Ble { .. } if ble::connected(&sensor) => ui::Calibration::Unsupported,
        settings::Sensor::Ant { .. } if ant::calibrate() => ui::Calibration::Running,
        _ => ui::Calibration::NotConnected,
    };
    info!("power meter calibration: {state:?}");
    CALIBRATION.lock(|c| c.set((state, Instant::now())));
}

/// The power meter calibration now: its answer, or no answer in time.
fn power_calibration() -> ui::Calibration {
    let (mut state, at) = CALIBRATION.lock(Cell::get);
    if state == ui::Calibration::Running {
        if let Some(answer) = ble::calibrated().or_else(ant::calibrated) {
            state = match answer {
                sensors::Calibration::Done(offset) => ui::Calibration::Done(offset),
                sensors::Calibration::Failed => ui::Calibration::Failed,
                sensors::Calibration::Unsupported => ui::Calibration::Unsupported,
            };
        } else if at.elapsed() >= CALIBRATION_WAIT {
            state = ui::Calibration::Timeout;
        }
        if state != ui::Calibration::Running {
            info!("power meter calibration: {state:?}");
            CALIBRATION.lock(|c| c.set((state, at)));
        }
    }
    state
}

/// The power of the last 3 s, fed by each frame drawn.
static POWER_3S: Mutex<CriticalSectionRawMutex, Cell<sensors::Avg3s>> = Mutex::new(Cell::new(sensors::Avg3s::new()));

/// The sensor values now: the BLE ones, else the ANT+ ones.
fn sensor_values() -> ble::Current {
    let (b, a) = (ble::current(), ant::current());
    ble::Current {
        heart_rate: b.heart_rate.or(a.heart_rate),
        cadence_rpm: b.cadence_rpm.or(a.cadence_rpm),
        speed_mm_s: b.speed_mm_s.or(a.speed_mm_s),
        power_w: b.power_w.or(a.power_w),
    }
}

/// Follows the loaded route, or sets it aside (saved: a ride resumed after a restart keeps it).
fn follow_route(follow: bool) {
    let mut s = settings();
    s.follow_route = follow;
    info!("route: {}", if follow { "followed" } else { "set aside (free ride)" });
    SETTINGS.lock(|c| c.set(Some(s)));
    SETTINGS_CHANGED.signal(());
}

/// The paired sensors of the settings, for the BLE task.
fn paired(s: &settings::Settings) -> ble::Paired {
    ble::Paired {
        hr: s.hr,
        speed_cadence: s.speed_cadence,
        power: s.power,
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "the file read in a 512-byte buffer, once at boot from main"
)]
fn load_settings(fs: &Fs<'_>) -> settings::Settings {
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
fn save_settings(fs: &Fs<'_>, s: &settings::Settings) {
    let mut text = String::new();
    let _ = s.write(&mut text);
    let path = littlefs2::path::PathBuf::try_from(SETTINGS_FILE).unwrap();
    match fs.write(&path, text.as_bytes()) {
        Ok(()) => info!("settings: saved"),
        Err(e) => warn!("settings: save failed, littlefs error {}", e.code()),
    }
}

/// What the companion app (BLE, [`ble::run`]) reads and changes: the settings, the route, the rides.
#[derive(Default)]
struct App {
    /// A ride read ahead: id, offset, bytes. The app reads 512 bytes at a time, each request waits for the main loop.
    ahead: Option<(u32, u32, Vec<u8>)>,
}

/// Bytes of a ride read from the storage for each request of the app (the most a frame carries).
const READ_AHEAD: u16 = protocol_cc600::MAX_PAYLOAD as u16;

/// What the app asks of the rides, served by the main loop (it owns the storage).
enum RideRequest {
    List,
    Read { id: u32, offset: u32, max: u16 },
    Sent(u32),
}

enum RideAnswer {
    List(Vec<protocol_cc600::payload::ActivityEntry>),
    Read(Vec<u8>),
    Sent,
}

static RIDE_REQUEST: Signal<CriticalSectionRawMutex, RideRequest> = Signal::new();
static RIDE_ANSWER: Signal<CriticalSectionRawMutex, Result<RideAnswer, protocol_cc600::ErrorCode>> =
    Signal::new();

/// One request at a time: the app waits for each answer.
async fn ask_rides(r: RideRequest) -> Result<RideAnswer, protocol_cc600::ErrorCode> {
    RIDE_ANSWER.reset();
    RIDE_REQUEST.signal(r);
    RIDE_ANSWER.wait().await
}

/// Seconds from the Unix epoch to the FIT one (1989-12-31T00:00:00Z).
const FIT_EPOCH_UNIX: u32 = 631_065_600;

fn serve_rides(fs: &Fs<'_>, r: RideRequest) -> Result<RideAnswer, protocol_cc600::ErrorCode> {
    use protocol_cc600::ErrorCode;
    match r {
        RideRequest::List => {
            let mut list = Vec::new();
            ride_store::finished_sent(fs, |id, size, sent| {
                list.push(protocol_cc600::payload::ActivityEntry {
                    id,
                    start_time: id.saturating_sub(FIT_EPOCH_UNIX),
                    size: size as u32,
                    synced: sent,
                })
            })
            .map_err(|_| ErrorCode::Storage)?;
            list.sort_by_key(|e| e.id);
            Ok(RideAnswer::List(list))
        }
        RideRequest::Read { id, offset, max } => {
            let mut buf = alloc::vec![0; usize::from(max)];
            match ride_store::read_at(fs, id, true, offset as usize, &mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    Ok(RideAnswer::Read(buf))
                }
                Err(Error::NO_SUCH_ENTRY) => Err(ErrorCode::BadRequest),
                Err(_) => Err(ErrorCode::Storage),
            }
        }
        RideRequest::Sent(id) => match ride_store::mark_sent(fs, id) {
            Ok(()) => {
                info!("ride {id}: sent");
                Ok(RideAnswer::Sent)
            }
            Err(Error::NO_SUCH_ENTRY) => Err(ErrorCode::BadRequest),
            Err(e) => {
                warn!("ride {id}: littlefs error {} marking it sent", e.code());
                Err(ErrorCode::Storage)
            }
        },
    }
}

impl companion::Device for App {
    fn settings(&self) -> settings::Settings {
        settings()
    }

    fn set_settings(&mut self, s: settings::Settings) {
        SETTINGS.lock(|c| c.set(Some(s)));
        SETTINGS_CHANGED.signal(());
    }

    fn max_file(&self, kind: Kind) -> Result<u32, protocol_cc600::ErrorCode> {
        match kind {
            Kind::Route => Ok(ROUTE_MAX as u32),
            // not stored yet (`bench-map` only)
            Kind::Map => Err(protocol_cc600::ErrorCode::BadRequest),
        }
    }

    /// Stored by the main loop, like a route sent over USB.
    fn file_received(&mut self, kind: Kind, data: Vec<u8>) -> Result<(), protocol_cc600::ErrorCode> {
        match kind {
            Kind::Route => {
                ROUTE_RECEIVED.signal(Some(data));
                Ok(())
            }
            Kind::Map => Err(protocol_cc600::ErrorCode::BadRequest),
        }
    }

    async fn rides(&mut self) -> Result<Vec<protocol_cc600::payload::ActivityEntry>, protocol_cc600::ErrorCode> {
        self.ahead = None;
        match ask_rides(RideRequest::List).await? {
            RideAnswer::List(list) => Ok(list),
            _ => Err(protocol_cc600::ErrorCode::Busy),
        }
    }

    async fn read_ride(&mut self, id: u32, offset: u32, max: u16) -> Result<Vec<u8>, protocol_cc600::ErrorCode> {
        let held = |a: &(u32, u32, Vec<u8>)| {
            a.0 == id && a.1 <= offset && offset + u32::from(max) <= a.1 + a.2.len() as u32
                // the end of the file: fewer bytes than asked for
                || a.0 == id && a.1 <= offset && a.2.len() < usize::from(READ_AHEAD)
        };
        if !self.ahead.as_ref().is_some_and(held) {
            self.ahead = None;
            let max = max.max(READ_AHEAD);
            match ask_rides(RideRequest::Read { id, offset, max }).await? {
                RideAnswer::Read(data) => self.ahead = Some((id, offset, data)),
                _ => return Err(protocol_cc600::ErrorCode::Busy),
            }
        }
        let (_, start, data) = self.ahead.as_ref().ok_or(protocol_cc600::ErrorCode::Busy)?;
        let from = ((offset - start) as usize).min(data.len());
        Ok(data[from..(from + usize::from(max)).min(data.len())].to_vec())
    }

    async fn ride_sent(&mut self, id: u32) -> Result<(), protocol_cc600::ErrorCode> {
        self.ahead = None;
        match ask_rides(RideRequest::Sent(id)).await? {
            RideAnswer::Sent => Ok(()),
            _ => Err(protocol_cc600::ErrorCode::Busy),
        }
    }
}

/// The route file, and the one being written (renamed over it once complete: a power cut keeps one of the two).
const ROUTE_FILE: &str = "/route";
const ROUTE_NEW: &str = "/route.new";
/// Largest route accepted (~9 000 points).
const ROUTE_MAX: usize = 64 * 1024;
/// A route being received over USB.
static ROUTE_RX: Mutex<CriticalSectionRawMutex, RefCell<Option<Vec<u8>>>> =
    Mutex::new(RefCell::new(None));
/// The test hook asks for the map benchmark.
static BENCH_MAP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// The test hook fills the storage (`storage fill N`: the logs deleted, until N blocks are left free) or empties it
/// again (`storage unfill`, `None`), with [`FILL_FILE`] only.
static STORAGE_FILL: Signal<CriticalSectionRawMutex, Option<u16>> = Signal::new();
const FILL_FILE: &str = "/fill.bin";
/// Blocks written into [`FILL_FILE`] per pass of the main loop (~1.3 s: 80 ms a block, measured).
const FILL_STEP: usize = 16;

/// Starts a `storage fill` (removes the file first): the blocks it leaves free.
#[allow(clippy::large_stack_frames, reason = "littlefs path and file handle on the main stack, a test hook")]
fn storage_fill_start(fs: &Fs<'_>, fill: Option<u16>) -> Option<usize> {
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
fn storage_fill(fs: &Fs<'_>, left: usize) -> Option<usize> {
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
/// A route received (`None`: remove the route).
static ROUTE_RECEIVED: Signal<CriticalSectionRawMutex, Option<Vec<u8>>> = Signal::new();

#[allow(
    clippy::large_stack_frames,
    reason = "littlefs file handle, once at boot from main"
)]
fn load_route(fs: &Fs<'_>) -> Option<Vec<u8>> {
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
fn store_route(fs: &Fs<'_>, data: Option<Vec<u8>>) -> Option<Vec<u8>> {
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
fn map_for(route_file: Option<&[u8]>) -> Option<route::map::Map<'static>> {
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
fn bench_map(fb: &mut FrameBuffer, r: &route::Route<'_>, m: Option<&route::map::Map<'_>>, view: &ui::View) {
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

/// The night theme is shown: the `theme` setting, or from sunset to sunrise with `auto_night`.
static NIGHT: AtomicBool = AtomicBool::new(false);
/// The backlight set, percent.
static BACKLIGHT: AtomicU8 = AtomicU8::new(0);

fn theme() -> ui::Theme {
    if NIGHT.load(Ordering::Relaxed) {
        ui::NIGHT
    } else {
        ui::DAY
    }
}

/// What the screen shows of the ride.
#[derive(Clone, Copy, Default, PartialEq)]
struct RideInfo {
    running: bool,
    timer_s: u32,
    elapsed_s: u32,
    distance_cm: u32,
    ascent_m: u16,
    descent_m: u16,
    max_speed_mm_s: u32,
    avg_power_w: Option<u16>,
    avg_heart_rate: Option<u8>,
    max_heart_rate: Option<u8>,
    /// Altitude along the ride (gradient, profiles).
    history: ui::history::History,
    /// The ride is no longer written.
    failure: Option<ui::WriteFailure>,
    /// A point every 5 s: the storage is nearly full.
    sparse: bool,
}

/// Why a ride did not start.
enum StartError {
    /// No GNSS fix nor RTC time yet.
    NoTime,
    /// Less than [`room::RIDE_START_BYTES`] free, without the logs.
    Full,
    /// Another littlefs error.
    Storage(i32),
}

/// The ride being recorded: one FIT record per GNSS epoch (1 Hz) while the timer runs.
struct Ride {
    recorder: recorder::Recorder,
    odometer: recorder::Odometer,
    /// GNSS time of the last epoch recorded.
    last_epoch: Option<gnss::Time>,
    clock: recorder::EpochClock,
    last_timestamp: u32,
    /// Altitude along the ride, in RAM (lost when the ride is resumed after a power off).
    history: ui::history::History,
    /// The last write failed even with the logs' room: when, and why. Nothing is written before [`RETRY_WRITE`].
    failure: Option<(Instant, ui::WriteFailure)>,
    /// When the room left was read last (`None`: at the next sample): below [`room::SPARSE_BYTES`] the ride records
    /// a point every [`recorder::SPARSE_INTERVAL_S`], to its end.
    room_read: Option<Instant>,
}

/// Room for the rides: free, and what the logs take (they give way).
fn ride_room(fs: &Fs<'_>) -> littlefs2::io::Result<usize> {
    Ok(room::free_bytes(fs)? + room::logs_bytes(fs)?)
}

/// A finished ride on the rides screen.
struct StoredRide {
    id: u32,
    sent: bool,
    totals: Option<fit::Totals>,
}

/// The finished rides, in the order of the rides screen: the latest first; to make room, the ones the app has first,
/// then the oldest first.
fn stored_rides(fs: &Fs<'_>, making_room: bool) -> Vec<StoredRide> {
    let t = Instant::now();
    let mut list = Vec::new();
    if let Err(e) = ride_store::finished_sent(fs, |id, size, sent| list.push((id, size, sent))) {
        warn!("rides: littlefs error {} listing", e.code());
    }
    let mut rides: Vec<StoredRide> = list
        .into_iter()
        .map(|(id, size, sent)| StoredRide { id, sent, totals: recorder::finished_totals(fs, id, size).ok() })
        .collect();
    if making_room {
        rides.sort_by_key(|r| (!r.sent, r.id));
    } else {
        rides.sort_by_key(|r| core::cmp::Reverse(r.id));
    }
    info!("rides: {} listed in {} ms", rides.len(), t.elapsed().as_millis());
    rides
}

fn ride_card(r: &StoredRide) -> ui::RideCard {
    let local = nrf_link::RtcTime::from_unix(i64::from(r.id) + i64::from(utc_offset(Some(r.id))));
    let t = r.totals.unwrap_or_default();
    ui::RideCard {
        weekday: local.weekday(),
        day: local.day,
        month: local.month,
        hour: local.hour,
        minute: local.minute,
        timer_s: t.timer_ms / 1000,
        distance_cm: t.distance_cm,
        sent: r.sent,
    }
}

type Fs<'a> = Filesystem<'a, storage::Partition<'a>>;

/// Raw sensor log, when asked for (`set trip_log on`): one CSV line per GNSS epoch (or every 2 s if the receiver is
/// silent), appended every 10 s to `/logs/<unix>.csv`, a new file every [`room::LOG_FILE_BYTES`]. The logs give
/// way to the rides ([`room`]): the oldest are deleted, and the log stops when there is no room. For replaying the
/// altitude filter and the GNSS on a PC, and a first battery-life record. Contains real positions: copy it to
/// `dump/` only.
struct TripLog {
    /// The file written, `None` until there is room for a new one.
    path: Option<littlefs2::path::PathBuf>,
    /// Written into `path`.
    file_bytes: usize,
    buf: String,
    last_epoch: Option<gnss::Time>,
    last_line: Instant,
    last_flush: Instant,
    started: Instant,
    start_unix: u32,
    lines: u32,
    /// The last write or the room for a new file failed: nothing tried before [`RETRY_WRITE`] (on a full storage,
    /// each try walks the whole file system).
    failed_at: Option<Instant>,
    /// The free room read last (the rides fill it too): below the reserve, the file is closed.
    room_read: Instant,
}

/// After a failed write (full storage), the next try waits this long.
const RETRY_WRITE: Duration = Duration::from_secs(60);

/// What the trip log writes a second (measured: 3.3 MB for a 5 h session).
const TRIP_LOG_BYTES_PER_S: u32 = 190;

const TRIP_LOG_HEADER: &str = "unix,gnss_unix,rtc_unix,uptime_s,fix,quality,sats_used,sats_in_view,hdop_c,lat_e7,\
lon_e7,gnss_alt_dm,speed_mm_s,course_cdeg,baro_c_celsius,pressures_cpa,altitude_dm,battery_mv,battery_pct,charger,\
light,ride,gnss_bytes,gnss_silences,gnss_setups,gnss_restarts,gnss_aiding_requests,gnss_time_aids,gnss_ttff_ms,\
keys,ble_links,ble_listening,ant_links\n";

impl TripLog {
    #[allow(clippy::large_stack_frames, reason = "the log (a littlefs path) built on the main stack, once")]
    fn start(s: &Status) -> Option<Self> {
        let start_unix = s.now_unix()?;
        let now = Instant::now();
        Some(Self {
            path: None,
            file_bytes: 0,
            buf: String::new(),
            last_epoch: None,
            last_line: now,
            last_flush: now,
            started: now,
            start_unix,
            lines: 0,
            failed_at: None,
            room_read: now,
        })
    }

    /// A new file, with its header, once there is room for it.
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle on the main stack, once a log file"
    )]
    fn new_file(&self, fs: &Fs<'_>) -> Option<littlefs2::path::PathBuf> {
        let t = Instant::now();
        match room::for_log(fs) {
            Ok(true) => {}
            Ok(false) => {
                warn!("trip log: no room out of the rides' reserve, off for now");
                return None;
            }
            Err(e) => {
                warn!("trip log: littlefs error {} making room", e.code());
                return None;
            }
        }
        let unix = self.start_unix + self.started.elapsed().as_secs() as u32;
        let path = room::log_path(unix);
        let res = if fs.exists(room::LOGS) { Ok(()) } else { fs.create_dir(room::LOGS) };
        if let Err(e) = res.and_then(|()| fs.write(&path, TRIP_LOG_HEADER.as_bytes())) {
            warn!("trip log: littlefs error {} creating /logs/{unix}.csv", e.code());
            return None;
        }
        info!("trip log: /logs/{unix}.csv ({} ms to make room)", t.elapsed().as_millis());
        Some(path)
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle and a Status copy on the main stack, once a second at most"
    )]
    fn sample(&mut self, fs: &Fs<'_>, s: &Status, ride: Option<RideInfo>) {
        let epoch = s.fix.and_then(|f| f.time);
        let new_epoch = epoch.is_some() && epoch != self.last_epoch;
        if !new_epoch && self.last_line.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_epoch = epoch;
        self.last_line = Instant::now();
        let pressures = STATUS.lock(|c| {
            let mut st = c.get();
            let p = (st.baro_recent, st.baro_recent_n);
            st.baro_recent_n = 0;
            c.set(st);
            p
        });
        let opt = |v: Option<i64>| v.map_or_else(String::new, |v| format!("{v}"));
        let f = s.fix.unwrap_or_default();
        let mut p = String::new();
        for (i, v) in pressures.0[..pressures.1 as usize].iter().enumerate() {
            if i > 0 {
                p.push(';');
            }
            p.push_str(&format!("{v}"));
        }
        let line = format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            opt(s.now_unix().map(i64::from)),
            opt(s.gnss_unix().map(i64::from)),
            opt(s.rtc_unix().map(i64::from)),
            self.started.elapsed().as_secs(),
            f.valid as u8,
            f.quality,
            f.sats_used,
            f.sats_in_view,
            opt(f.hdop_c.map(i64::from)),
            opt(f.lat_e7.map(i64::from)),
            opt(f.lon_e7.map(i64::from)),
            opt(f.alt_dm.map(i64::from)),
            opt(f.speed_mm_s.map(i64::from)),
            opt(f.course_cdeg.map(i64::from)),
            opt(s.baro.map(|(c, _)| i64::from(c))),
            p,
            opt(s.altimeter.altitude_dm().map(i64::from)),
            opt(s.battery.map(|(mv, _)| i64::from(mv))),
            opt(s.battery.map(|(_, pct)| i64::from(pct))),
            opt(s.charger.map(i64::from)),
            opt(s.light[0].map(i64::from)),
            match ride {
                None => 0,
                Some(r) if r.running => 1,
                Some(_) => 2,
            },
            s.gnss.map_or(0, |g| g.bytes),
            s.gnss_link.silences,
            s.gnss_link.setups,
            s.gnss_link.restarts,
            s.gnss_link.aiding_requests,
            s.gnss_link.time_aids,
            opt(s.gnss_link.ttff_ms.map(i64::from)),
            KEY_LOG.lock(|l| core::mem::take(&mut *l.borrow_mut())),
            ble::links(),
            ble::listening() as u8,
            ant::links(),
        );
        self.buf.push_str(&line);
        self.lines += 1;
        if self.last_flush.elapsed() >= Duration::from_secs(10) || self.buf.len() > 2048 {
            self.flush(fs);
        }
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle and path on the main stack, every 10 s"
    )]
    fn flush(&mut self, fs: &Fs<'_>) {
        self.last_flush = Instant::now();
        if self.failed_at.is_some_and(|t| t.elapsed() < RETRY_WRITE) {
            if self.buf.len() > 16 * 1024 {
                self.buf.clear();
            }
            return;
        }
        self.failed_at = None;
        // a walk through the file system (~30 ms): once a minute
        if self.path.is_some() && self.room_read.elapsed() >= RETRY_WRITE {
            self.room_read = Instant::now();
            if room::free_bytes(fs).is_ok_and(|free| free < room::RESERVE_BYTES) {
                info!("trip log: in the rides' reserve, a new file once there is room");
                self.path = None;
            }
        }
        if self.path.is_none() {
            self.path = self.new_file(fs);
            self.file_bytes = 0;
        }
        let Some(path) = self.path.as_ref() else {
            self.failed_at = Some(Instant::now());
            return;
        };
        let res = fs.open_file_with_options_and_then(
            |o| o.write(true).append(true),
            path,
            |file| {
                let mut rest = self.buf.as_bytes();
                while !rest.is_empty() {
                    let n = file.write(rest)?;
                    rest = &rest[n..];
                }
                Ok(())
            },
        );
        match res {
            Ok(()) => {
                if self.lines % 60 < 10 {
                    info!("trip log: {} lines", self.lines);
                }
                self.file_bytes += self.buf.len();
                self.buf.clear();
                if self.file_bytes >= room::LOG_FILE_BYTES {
                    self.path = None;
                }
            }
            Err(e) => {
                // storage full or broken: keep running, a new file (room made first) in a while
                warn!("trip log: littlefs error {}, tried again in {} s", e.code(), RETRY_WRITE.as_secs());
                self.failed_at = Some(Instant::now());
                self.path = None;
                if self.buf.len() > 16 * 1024 {
                    self.buf.clear();
                }
            }
        }
    }
}

impl Ride {
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handles (cache included) on the main stack, once per ride"
    )]
    fn start(fs: &Fs<'_>, s: &Status) -> Result<Box<Self>, StartError> {
        let Some(unix) = s.now_unix() else {
            warn!("ride: no time yet (no GNSS fix, no RTC), not started");
            return Err(StartError::NoTime);
        };
        // the logs give way to the rides
        match room::for_ride(fs) {
            Ok(free) if free < room::RIDE_START_BYTES => {
                warn!("ride: {free} bytes free without the logs, not started");
                return Err(StartError::Full);
            }
            Ok(_) => {}
            Err(e) => {
                warn!("ride: littlefs error {} making room, not started", e.code());
                return Err(StartError::Storage(e.code()));
            }
        }
        match recorder::Recorder::start(fs, unix, fit::timestamp(unix), serial()) {
            Ok(recorder) => {
                info!("ride {unix}: started");
                sound(SOUND_BEEP);
                Ok(Box::new(Self {
                    recorder,
                    odometer: recorder::Odometer::default(),
                    last_epoch: None,
                    clock: recorder::EpochClock::default(),
                    last_timestamp: fit::timestamp(unix),
                    history: ui::history::History::new(),
                    failure: None,
                    room_read: None,
                }))
            }
            Err(e) => {
                warn!("ride: start failed, littlefs error {}", e.code());
                Err(if e == Error::NO_SPACE { StartError::Full } else { StartError::Storage(e.code()) })
            }
        }
    }

    /// Ride `id` opened again, paused: interrupted (`finished` false) or finished (the end screen's "resume").
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handles (cache included) on the main stack, once per ride"
    )]
    fn reopen(fs: &Fs<'_>, id: u32, finished: bool) -> Option<Box<Self>> {
        let res = if finished {
            recorder::Recorder::reopen_finished(fs, id)
        } else {
            recorder::Recorder::reopen(fs, id)
        };
        match res {
            Ok(recorder) => {
                let t = recorder.summary().totals();
                info!("ride {id}: reopened paused, {t:?}");
                Some(Box::new(Self {
                    odometer: recorder::Odometer::starting_at(t.distance_cm),
                    last_epoch: None,
                    clock: recorder::EpochClock::default(),
                    last_timestamp: t.end_time,
                    history: ui::history::History::new(),
                    failure: None,
                    room_read: None,
                    recorder,
                }))
            }
            Err(e) => {
                warn!("ride {id}: reopen failed, littlefs error {}", e.code());
                None
            }
        }
    }

    /// Writes what is buffered, before a power off: the ride (paused) is offered again at power-on.
    fn keep(&mut self, fs: &Fs<'_>) {
        let res = self.recorder.flush(fs);
        self.wrote(fs, res);
        if self.failure.is_none() {
            info!("ride {}: kept for the next power-on", self.recorder.id());
        }
    }

    /// Follows a write: on a full storage the logs' room is taken and what is buffered written again. If that fails
    /// too (or on another error), the ride is not written for [`RETRY_WRITE`] (the screen says so).
    fn wrote(&mut self, fs: &Fs<'_>, res: littlefs2::io::Result<()>) {
        let mut res = res;
        if res == Err(Error::NO_SPACE) {
            let t = Instant::now();
            match room::for_ride(fs) {
                Ok(free) => {
                    info!("ride: storage full, {free} bytes free without the logs ({} ms)", t.elapsed().as_millis())
                }
                Err(e) => warn!("ride: littlefs error {} making room", e.code()),
            }
            res = self.recorder.flush(fs);
        }
        match (res, self.failure) {
            (Ok(()), None) => {}
            (Ok(()), Some(_)) => {
                info!("ride: written again");
                self.failure = None;
            }
            (Err(e), _) => {
                warn!(
                    "ride: write failed, littlefs error {}, not tried again for {} s",
                    e.code(),
                    RETRY_WRITE.as_secs()
                );
                let why = if e == Error::NO_SPACE { ui::WriteFailure::Full } else { ui::WriteFailure::Error };
                self.failure = Some((Instant::now(), why));
            }
        }
    }

    fn toggle_pause(&mut self, fs: &Fs<'_>, s: &Status) {
        let t = s.now_unix()
            .map_or(self.last_timestamp, fit::timestamp)
            .max(self.last_timestamp);
        let running = self.recorder.summary().running();
        let res = if running {
            self.recorder.pause(fs, t)
        } else {
            self.recorder.resume(fs, t)
        };
        self.wrote(fs, res);
        // the timer follows the key anyway: only the event may be missing from the file
        info!("ride: {}", if running { "paused" } else { "resumed" });
        if self.failure.is_none() {
            sound(SOUND_BEEP);
        }
    }

    /// Records the latest GNSS epoch if it is new.
    fn sample(&mut self, fs: &Fs<'_>, s: &Status) {
        let Some(fix) = s.fix else { return };
        if fix.time.is_none() || fix.time == self.last_epoch {
            return;
        }
        self.last_epoch = fix.time;
        let Some(epoch) = fix.time else { return };
        let epoch_s = epoch.hour as u32 * 3600 + epoch.minute as u32 * 60 + epoch.second as u32;
        let gnss = s.gnss_unix().map(fit::timestamp);
        let rtc = s.rtc_unix().map(fit::timestamp);
        let Some(timestamp) = self.clock.next(epoch_s, gnss, rtc) else {
            return;
        };
        self.last_timestamp = timestamp;
        let speed = fix.valid.then_some(fix.speed_mm_s).flatten();
        let distance_cm = self
            .odometer
            .update(timestamp, speed, self.recorder.summary().running());
        let sensors = sensor_values();
        let record = fit::Record {
            timestamp,
            lat_e7: fix.lat_e7,
            lon_e7: fix.lon_e7,
            // the barometer calibrated on the GNSS; the raw GNSS altitude until then, or without a barometer
            altitude_dm: s
                .altimeter
                .altitude_dm()
                .or(fix.valid.then_some(fix.alt_dm).flatten()),
            speed_mm_s: speed,
            distance_cm: Some(distance_cm),
            temperature_c: s.baro.map(|(c, _)| (c / 100) as i8),
            pressure_pa: s.baro.map(|(_, p)| (p + 50) / 100),
            heart_rate: sensors.heart_rate,
            cadence: sensors.cadence_rpm,
            power_w: sensors.power_w,
        };
        // a point every 5 s once the storage is nearly full (a walk through the file system: once a minute)
        if self.recorder.interval_s() == 1 && self.room_read.is_none_or(|t| t.elapsed() >= RETRY_WRITE) {
            self.room_read = Some(Instant::now());
            if let Ok(left) = ride_room(fs)
                && left < room::SPARSE_BYTES
            {
                info!("ride: {left} bytes left for the rides, a point every {} s", recorder::SPARSE_INTERVAL_S);
                self.recorder.set_interval(recorder::SPARSE_INTERVAL_S);
            }
        }
        // after a failure, what is buffered is written first, every [`RETRY_WRITE`]; the records meanwhile are lost
        let failing = match self.failure {
            Some((at, _)) if at.elapsed() < RETRY_WRITE => true,
            Some(_) => {
                let res = self.recorder.flush(fs);
                self.wrote(fs, res);
                self.failure.is_some()
            }
            None => false,
        };
        if !failing {
            let res = self.recorder.record(fs, &record);
            self.wrote(fs, res);
        }
        if let (true, Some(alt)) = (self.recorder.summary().running(), record.altitude_dm) {
            self.history.push(distance_cm, alt);
            self.history.push_time(self.recorder.summary().totals().timer_ms / 1000, alt);
        }
    }

    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handles and the recorder on the main stack, once per ride"
    )]
    fn stop(mut self: Box<Self>, fs: &Fs<'_>, s: &Status) {
        let id = self.recorder.id();
        // the room for what is buffered and the closing messages
        let res = self.recorder.flush(fs);
        self.wrote(fs, res);
        let t = s.now_unix()
            .map_or(self.last_timestamp, fit::timestamp)
            .max(self.last_timestamp);
        match self.recorder.stop(fs, t, utc_offset(s.now_unix())) {
            Ok(totals) => {
                info!("ride {id}: saved, {totals:?}");
                sound(command::SOUND_JINGLE);
            }
            Err(e) => warn!(
                "ride {id}: stop failed, littlefs error {} (closed at next boot)",
                e.code()
            ),
        }
    }

    fn info(&self) -> RideInfo {
        let t = self.recorder.summary().totals();
        RideInfo {
            running: self.recorder.summary().running(),
            timer_s: t.timer_ms / 1000,
            elapsed_s: t.elapsed_ms / 1000,
            distance_cm: self.odometer.distance_cm(),
            ascent_m: t.ascent_m,
            descent_m: t.descent_m,
            max_speed_mm_s: t.max_speed_mm_s,
            avg_power_w: self.recorder.summary().avg_power_w(),
            avg_heart_rate: self.recorder.summary().avg_heart_rate(),
            max_heart_rate: self.recorder.summary().max_heart_rate(),
            history: self.history,
            failure: self.failure.map(|(_, why)| why),
            sparse: self.recorder.interval_s() > 1,
        }
    }
}

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
/// Short beep on the buzzer (`E2 01 00`).
const SOUND_BEEP: u8 = 0x00;
/// Buzzer on; `sound off` on the USB test hook mutes it (tests next to people).
static SOUND_ON: AtomicBool = AtomicBool::new(true);
/// Test hook (`power off dry`, until the next boot): a power off restarts the ESP32 instead of asking the nRF to cut
/// it, which would stop the nRF's sensor stream (time, barometer) until a long press of the power key.
static DRY_POWER_OFF: AtomicBool = AtomicBool::new(false);
/// Test hook (`battery fake N`, `battery real`): the battery level shown, not the one measured (the charging screen's
/// animation at full charge).
static FAKE_BATTERY: AtomicU8 = AtomicU8::new(FAKE_NONE);
const FAKE_NONE: u8 = u8::MAX;
/// Test hook (`gnss off N`): the receiver power-ons to drop.
static GNSS_POWER_LOST: AtomicU8 = AtomicU8::new(0);
/// The screen shown, for the test hook, with what is selected on it: the menu entry, the sensor role, the entry
/// of the scan screen, the ride page.
type Shown = (ui::Screen, Option<ui::Item>, ui::Role, u8, Option<ui::Page>);
static SCREEN: Mutex<CriticalSectionRawMutex, Cell<Shown>> =
    Mutex::new(Cell::new((ui::Screen::Home, None, ui::Role::HeartRate, 0, None)));
/// The ride selected on the rides screen (its id, 0 for none), for the test hook: a test deletes its own ride only.
static SHOWN_RIDE: AtomicU32 = AtomicU32::new(0);

fn sound(id: u8) {
    if SOUND_ON.load(Ordering::Relaxed) {
        let _ = NRF_COMMANDS.try_send(command::misc(id));
    }
}

/// What the screens show (crates/ui).
#[allow(
    clippy::large_stack_frames,
    reason = "the view (two altitude profiles) and the status, from main"
)]
fn view_of(s: &Status, ride: &RideInfo, resumable: bool) -> ui::View {
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

/// The test hook: the commands typed on the USB serial port (crates/test-hook).
#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: line buffer, statically allocated by embassy"
)]
async fn usb_keys(mut rx: UsbSerialJtagRx<'static, esp_hal::Blocking>) {
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
                        "light: theme {}, backlight {} %",
                        if NIGHT.load(Ordering::Relaxed) { "night" } else { "day" },
                        BACKLIGHT.load(Ordering::Relaxed)
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
fn sim_fix(s: &Status, at: test_hook::SimFix) -> gnss::Fix {
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

/// Centred box over the current screen.
fn draw_message(fb: &mut FrameBuffer, text: &str) {
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
fn draw_status(fb: &mut FrameBuffer, s: &Status, ride: Option<RideInfo>) {
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
fn draw_test_pattern(fb: &mut FrameBuffer) {
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
fn bench(panel: &mut Panel, fb: &mut FrameBuffer) {
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
