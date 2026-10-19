//! Bring-up firmware (docs/PRODUCT.md, technical plan).
//!
//! Step 0: an esp-hal image in `ota_0` boots under the vendor bootloader, with the 2 MB quad PSRAM.
//! Panel: ST7789 on the i8080 16-bit bus, test pattern drawn with `embedded-graphics`.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
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
use log::{debug, info, warn};
use nrf_link::{Decoder, KeyAction, Message, RtcTime, command};
use opencc600_firmware::gnss_uart::{self, GnssUart};
#[cfg(feature = "hil-storage")]
use opencc600_firmware::hil_storage;
use opencc600_firmware::panel::{self, FrameBuffer, Panel};
use opencc600_firmware::storage;
use static_cell::StaticCell;

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// App descriptor read by the ESP-IDF second-stage bootloader (the vendor one stays in place).
esp_bootloader_esp_idf::esp_app_desc!();

/// Backlight on GPIO45, LEDC low-speed timer 0 / channel 0, 10-bit, 20 kHz, active high
/// (c606-oss `main/backlight.c`, mirrors the vendor `MidLcdPwmInit()`).
const BACKLIGHT_PCT: u8 = 50;

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

    info!("opencc600: booted from ota_0");
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

    // Storage: our littlefs partition, formatted here only if blank (tools/flash_ota0.py --erase-storage).
    let mut partition = storage::Partition::new(FlashStorage::new(peripherals.FLASH));
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
    // Rides interrupted by a power cut or a crash: closed with what had been written.
    if let Ok(fs) = fs.as_ref()
        && let Err(e) = recorder::recover(fs, UTC_OFFSET_S, |r| info!("ride recovered: {r:?}"))
    {
        warn!("ride recovery: littlefs error {}", e.code());
    }
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

    let mut fb = FrameBuffer::new();
    draw_test_pattern(&mut fb);
    panel.flush(&fb);
    info!("test pattern drawn");

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
    let mut bl = ledc.channel(channel::Number::Channel0, peripherals.GPIO45);
    bl.configure(channel::config::Config {
        timer: &bl_timer,
        duty_pct: BACKLIGHT_PCT,
        drive_mode: DriveMode::PushPull,
    })
    .unwrap();
    info!("backlight on ({BACKLIGHT_PCT} %)");

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
    let mut last_power_on = Instant::now();
    let mut ui = ui::Ui::new();
    let mut ride: Option<Box<Ride>> = None;
    // the ride in progress, else the last one saved (the "saved" screen)
    let mut ride_shown = RideInfo::default();
    let mut trip_log: Option<TripLog> = None;
    loop {
        let _ = with_timeout(Duration::from_secs(1), STATUS_CHANGED.wait()).await;
        // The vendor repeats SendPowerOnCmd; c606-oss does it every 5 s. No nRF watchdog seen so far.
        if last_power_on.elapsed() >= Duration::from_secs(5) {
            NRF_COMMANDS.send(command::power_on()).await;
            last_power_on = Instant::now();
        }
        // What each key does is up to the screen (crates/ui): bottom right starts / pauses / resumes, a ride ends
        // from the pause screen, power off is a long press on the top left key confirmed by a click on it.
        let mut commands = Vec::new();
        while let Ok((key, action)) = NRF_KEYS.try_receive() {
            let press = match action {
                KeyAction::Click => ui::Press::Click,
                KeyAction::LongPress => ui::Press::Long,
                KeyAction::LongRelease | KeyAction::Other(_) => continue,
            };
            let Some(key) = ui::Key::from_nrf(key) else {
                continue;
            };
            if let Some(c) = ui.key(key, press, Instant::now().as_millis()) {
                commands.push(c);
            }
        }
        let state = STATUS.lock(Cell::get);
        for c in commands {
            match (c, ride.as_mut(), fs.as_ref()) {
                (ui::Command::PowerOff, _, _) => {
                    if let (Some(r), Ok(fs)) = (ride.take(), fs.as_ref()) {
                        r.stop(fs, &state);
                    }
                    info!("power off");
                    draw_message(&mut fb, "Power off");
                    panel.flush(&fb);
                    NRF_COMMANDS.send(command::power_off()).await;
                    Timer::after(Duration::from_secs(2)).await;
                    warn!("still powered 2 s after power_off");
                }
                (ui::Command::Start, None, Ok(fs)) => {
                    ride = Ride::start(fs, &state);
                    if ride.is_none() && now_unix(&state).is_none() {
                        ui.set_notice(ui::NOTICE_NO_TIME, Instant::now().as_millis());
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
                    r.stop(fs, &state);
                    let storage_info = storage_summary(fs);
                    STATUS.lock(|s| {
                        let mut status = s.get();
                        status.storage = Some(storage_info);
                        s.set(status);
                    });
                }
                (c, _, _) => warn!("ui: {c:?} ignored"),
            }
        }
        if let Ok(fs) = fs.as_ref() {
            if let Some(r) = ride.as_mut() {
                r.sample(fs, &state);
            }
            if trip_log.is_none() {
                trip_log = TripLog::start(fs, &state);
            }
            if let Some(log) = trip_log.as_mut() {
                log.sample(fs, &state, ride.as_deref().map(Ride::info));
            }
        }
        let ride_info = ride.as_deref().map(Ride::info);
        ui.tick(Instant::now().as_millis(), ride_info.map(|r| r.running));
        if let Some(r) = ride_info {
            ride_shown = r;
        }
        let view = view_of(&state, ride_shown);
        // the status screen shows (nearly) all of the status, the others only what is in the view
        let status_shown = (ui.screen() == ui::Screen::Status).then_some(state);
        let screen = (ui, view, status_shown);
        if shown != Some(screen) {
            ui::draw(&mut fb, &ui, &view, &THEME).unwrap();
            if status_shown.is_some() {
                draw_status(&mut fb, &state, ride_info);
            }
            panel.flush(&fb);
            shown = Some(screen);
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
        // At most 5 redraws per second; the nRF task keeps updating STATUS meanwhile, nothing is lost.
        Timer::after(Duration::from_millis(200)).await;
    }
}

const RUN_BENCH: bool = false;

/// Latest state from the nRF, written by the nRF task, read by the display loop.
static STATUS: Mutex<CriticalSectionRawMutex, Cell<Status>> = Mutex::new(Cell::new(Status::new()));
static STATUS_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static NRF_KEYS: Channel<CriticalSectionRawMutex, (u8, KeyAction), 8> = Channel::new();

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
            match msg {
                Message::Unknown => info!("nrf ? {frame:?}"),
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
                _ => info!("nrf {msg:?}"),
            }
            if let Message::Key { key, action } = msg {
                let _ = NRF_KEYS.try_send((key, action));
            }
            STATUS.lock(|s| {
                let mut status = s.get();
                status.update(msg);
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
    let mut last_data = Instant::now();
    let mut silent = false;
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
        if n > 0 {
            last_data = Instant::now();
            silent = false;
        } else if !silent && last_data.elapsed() >= Duration::from_secs(3) {
            silent = true;
            link.silences += 1;
            // The nRF powers the receiver and can leave it off after a power cycle (seen once). Only when silent:
            // `gps_power(1)` restarts a running receiver, which then loses its output setup.
            warn!("gnss: no data for 3 s (receiver off, or DMA stopped), powering it on");
            NRF_COMMANDS.send(command::gps_power(1)).await;
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
        if epoch {
            let calibration = STATUS.lock(|s| {
                let mut status = s.get();
                status.fix = Some(fix);
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
                if silent { " SILENT" } else { "" }
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

/// GNSS receiver events since boot, for the trip log and the time-to-first-fix measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GnssLink {
    /// 3 s without data (receiver off or DMA stopped), each answered by `gps_power(1)`.
    silences: u16,
    /// Output setups sent (at boot, then after each receiver restart).
    setups: u16,
    /// `PAIR011,001` seen: the receiver (re)started.
    restarts: u16,
    /// `PAIR010` aiding requests (EPO, time, position).
    aiding_requests: u16,
    /// `PAIR590` time aiding sent.
    time_aids: u16,
    /// Time to the first valid fix since the last start (ESP32 boot, restart seen, or commanded), ms.
    ttff_ms: Option<u32>,
}

impl GnssLink {
    const fn new() -> Self {
        Self {
            silences: 0,
            setups: 0,
            restarts: 0,
            aiding_requests: 0,
            time_aids: 0,
            ttff_ms: None,
        }
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

/// What the bring-up screen shows, updated from the nRF messages.
#[derive(Clone, Copy, PartialEq)]
struct Status {
    battery: Option<(u16, u8)>,
    charger: Option<u8>,
    rtc: Option<RtcTime>,
    version: Option<(u8, u8, u8)>,
    power_on_reason: Option<u8>,
    last_key: Option<(u8, KeyAction)>,
    keys: u32,
    baro: Option<(i16, u32)>,
    /// Barometer altitude, its bias calibrated on the GNSS (docs/ALTITUDE.md); fed by each pressure frame (5 Hz)
    /// and each GNSS epoch.
    altimeter: altitude::Altimeter,
    baro_at: Option<Instant>,
    /// Pressures (centi-pascals) received since the trip log took them (5 Hz frames, logged once a second).
    baro_recent: [u32; 8],
    baro_recent_n: u8,
    /// Ambient light (raw) per measurement tag (`param` 600, 2000, 3000), and the raw sensor stream subtype `0A`
    /// (meaning unknown).
    light: [Option<u16>; 3],
    stream0a: Option<[u8; 6]>,
    /// Storage partition: Ok((finished rides, free blocks, total blocks)) or the littlefs error code.
    storage: Option<Result<(u16, u16, u16), i32>>,
    /// GNSS link counters and latest complete fix.
    gnss: Option<gnss::Stats>,
    gnss_link: GnssLink,
    fix: Option<gnss::Fix>,
}

impl Status {
    const fn new() -> Self {
        Self {
            battery: None,
            charger: None,
            rtc: None,
            version: None,
            power_on_reason: None,
            last_key: None,
            keys: 0,
            baro: None,
            altimeter: altitude::Altimeter::new(altitude::Tuning::DEFAULT),
            baro_at: None,
            baro_recent: [0; 8],
            baro_recent_n: 0,
            light: [None; 3],
            stream0a: None,
            storage: None,
            gnss: None,
            gnss_link: GnssLink::new(),
            fix: None,
        }
    }

    fn update(&mut self, msg: Message) {
        match msg {
            Message::Battery {
                millivolts,
                percent,
            } => self.battery = Some((millivolts, percent)),
            Message::Status(s) => self.charger = Some(s),
            Message::Rtc(t) => self.rtc = Some(t),
            Message::Version {
                major,
                minor,
                patch,
            } => self.version = Some((major, minor, patch)),
            Message::PowerOnReason(r) => self.power_on_reason = Some(r),
            Message::Key { key, action } => {
                self.last_key = Some((key, action));
                self.keys += 1;
            }
            Message::Baro {
                centi_celsius,
                centi_pascal,
            } => {
                self.baro = Some((centi_celsius, centi_pascal));
                let now = Instant::now();
                let dt = self
                    .baro_at
                    .map_or(0.0, |at| (now - at).as_micros() as f64 / 1e6);
                self.baro_at = Some(now);
                self.altimeter.baro(dt, centi_pascal as f64 / 100.0);
                if (self.baro_recent_n as usize) < self.baro_recent.len() {
                    self.baro_recent[self.baro_recent_n as usize] = centi_pascal;
                    self.baro_recent_n += 1;
                }
            }
            Message::Light { level, param } => {
                if let Some(i) = [600, 2000, 3000].iter().position(|&p| p == param) {
                    self.light[i] = Some(level);
                }
            }
            Message::Stream { sub: 0x0A, data } => self.stream0a = Some(data),
            Message::Ack
            | Message::Accel(_)
            | Message::Gyro(_)
            | Message::Stream { .. }
            | Message::Unknown => {}
        }
    }
}

/// Finished rides, free blocks, total blocks.
fn storage_summary(fs: &Filesystem<'_, storage::Partition<'_>>) -> Result<(u16, u16, u16), i32> {
    let mut rides = 0u16;
    ride_store::finished(fs, |_, _| rides += 1).map_err(|e| e.code())?;
    let free = fs.available_blocks().map_err(|e| e.code())? as u16;
    let total = fs.total_blocks() as u16;
    info!("storage: {rides} rides, {free} / {total} blocks free");
    Ok((rides, free, total))
}

/// Local time offset written in the FIT activity (`local_timestamp`). *To make a setting.*
const UTC_OFFSET_S: i32 = 0;

/// What the screen shows of the ride.
#[derive(Clone, Copy, Default, PartialEq)]
struct RideInfo {
    running: bool,
    timer_s: u32,
    distance_cm: u32,
    ascent_m: u16,
    descent_m: u16,
    max_speed_mm_s: u32,
}

/// The ride being recorded: one FIT record per GNSS epoch (1 Hz) while the timer runs.
struct Ride {
    recorder: recorder::Recorder,
    odometer: recorder::Odometer,
    /// GNSS time of the last epoch recorded.
    last_epoch: Option<gnss::Time>,
    clock: recorder::EpochClock,
    last_timestamp: u32,
}

type Fs<'a> = Filesystem<'a, storage::Partition<'a>>;

/// Raw sensor log, always on: one CSV line per GNSS epoch (or every 2 s if the receiver is silent) in
/// `/logs/<start unix>.csv`, appended every 10 s. For replaying the altitude filter and the GNSS on a PC, and a
/// first battery-life record. Contains real positions: copy it to `dump/` only.
struct TripLog {
    path: littlefs2::path::PathBuf,
    buf: String,
    last_epoch: Option<gnss::Time>,
    last_line: Instant,
    last_flush: Instant,
    started: Instant,
    lines: u32,
}

const TRIP_LOG_HEADER: &str = "unix,gnss_unix,rtc_unix,uptime_s,fix,quality,sats_used,sats_in_view,hdop_c,lat_e7,\
lon_e7,gnss_alt_dm,speed_mm_s,course_cdeg,baro_c_celsius,pressures_cpa,altitude_dm,battery_mv,battery_pct,charger,\
light,ride,gnss_bytes,gnss_silences,gnss_setups,gnss_restarts,gnss_aiding_requests,gnss_time_aids,gnss_ttff_ms\n";

impl TripLog {
    #[allow(
        clippy::large_stack_frames,
        reason = "littlefs file handle and a Status copy on the main stack, once a second at most"
    )]
    fn start(fs: &Fs<'_>, s: &Status) -> Option<Self> {
        let unix = now_unix(s)?;
        let dir = littlefs2::path!("/logs");
        if !fs.exists(dir) && fs.create_dir(dir).is_err() {
            return None;
        }
        let path = littlefs2::path::PathBuf::try_from(format!("/logs/{unix}.csv").as_str()).ok()?;
        info!("trip log: /logs/{unix}.csv");
        let now = Instant::now();
        Some(Self {
            path,
            buf: String::from(TRIP_LOG_HEADER),
            last_epoch: None,
            last_line: now,
            last_flush: now,
            started: now,
            lines: 0,
        })
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
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            opt(now_unix(s).map(i64::from)),
            opt(gnss_unix(s).map(i64::from)),
            opt(rtc_unix(s).map(i64::from)),
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
        );
        self.buf.push_str(&line);
        self.lines += 1;
        if self.last_flush.elapsed() >= Duration::from_secs(10) || self.buf.len() > 2048 {
            self.flush(fs);
        }
    }

    fn flush(&mut self, fs: &Fs<'_>) {
        self.last_flush = Instant::now();
        let res = fs.open_file_with_options_and_then(
            |o| o.write(true).create(true).append(true),
            &self.path,
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
                self.buf.clear();
            }
            Err(e) => {
                warn!("trip log: littlefs error {}", e.code());
                if self.buf.len() > 16 * 1024 {
                    self.buf.clear(); // storage full or broken: keep running
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
    fn start(fs: &Fs<'_>, s: &Status) -> Option<Box<Self>> {
        let Some(unix) = now_unix(s) else {
            warn!("ride: no time yet (no GNSS fix, no RTC), not started");
            return None;
        };
        match recorder::Recorder::start(fs, unix, fit::timestamp(unix), SERIAL) {
            Ok(recorder) => {
                info!("ride {unix}: started");
                sound(SOUND_BEEP);
                Some(Box::new(Self {
                    recorder,
                    odometer: recorder::Odometer::default(),
                    last_epoch: None,
                    clock: recorder::EpochClock::default(),
                    last_timestamp: fit::timestamp(unix),
                }))
            }
            Err(e) => {
                warn!("ride: start failed, littlefs error {}", e.code());
                None
            }
        }
    }

    fn toggle_pause(&mut self, fs: &Fs<'_>, s: &Status) {
        let t = now_unix(s)
            .map_or(self.last_timestamp, fit::timestamp)
            .max(self.last_timestamp);
        let running = self.recorder.summary().running();
        let res = if running {
            self.recorder.pause(fs, t)
        } else {
            self.recorder.resume(fs, t)
        };
        match res {
            Ok(()) => {
                info!("ride: {}", if running { "paused" } else { "resumed" });
                sound(SOUND_BEEP);
            }
            Err(e) => warn!("ride: pause/resume failed, littlefs error {}", e.code()),
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
        let gnss = gnss_unix(s).map(fit::timestamp);
        let rtc = rtc_unix(s).map(fit::timestamp);
        let Some(timestamp) = self.clock.next(epoch_s, gnss, rtc) else {
            return;
        };
        self.last_timestamp = timestamp;
        let speed = fix.valid.then_some(fix.speed_mm_s).flatten();
        let distance_cm = self.odometer.update(timestamp, speed);
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
            ..Default::default()
        };
        if let Err(e) = self.recorder.record(fs, &record) {
            warn!("ride: record failed, littlefs error {}", e.code());
        }
    }

    fn stop(self: Box<Self>, fs: &Fs<'_>, s: &Status) {
        let id = self.recorder.id();
        let t = now_unix(s)
            .map_or(self.last_timestamp, fit::timestamp)
            .max(self.last_timestamp);
        match self.recorder.stop(fs, t, UTC_OFFSET_S) {
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
            distance_cm: self.odometer.distance_cm(),
            ascent_m: t.ascent_m,
            descent_m: t.descent_m,
            max_speed_mm_s: t.max_speed_mm_s,
        }
    }
}

/// `file_id.serial_number` of the FIT files. *To read from the device (MAC address).*
const SERIAL: u32 = 600;
/// Short beep on the buzzer (`E2 01 00`).
const SOUND_BEEP: u8 = 0x00;
/// Buzzer on; `sound off` on the USB test hook mutes it (tests next to people).
static SOUND_ON: AtomicBool = AtomicBool::new(true);

fn sound(id: u8) {
    if SOUND_ON.load(Ordering::Relaxed) {
        let _ = NRF_COMMANDS.try_send(command::misc(id));
    }
}

/// Current time, Unix seconds: the GNSS time with a valid fix, else the nRF RTC.
fn now_unix(s: &Status) -> Option<u32> {
    gnss_unix(s).or_else(|| rtc_unix(s))
}

/// GNSS time, only with a valid fix (without, it can be off by tens of seconds).
fn gnss_unix(s: &Status) -> Option<u32> {
    let fix = s.fix?;
    let (true, Some(time), Some(date)) = (fix.valid, fix.time, fix.date) else {
        return None;
    };
    unix_of(&RtcTime {
        year: date.year,
        month: date.month,
        day: date.day,
        hour: time.hour,
        minute: time.minute,
        second: time.second,
    })
}

fn rtc_unix(s: &Status) -> Option<u32> {
    unix_of(&s.rtc?)
}

fn unix_of(t: &RtcTime) -> Option<u32> {
    u32::try_from(t.unix())
        .ok()
        .filter(|&u| u > fit::FIT_EPOCH_OFFSET)
}

/// Colours of the screens. *To make a setting (day / night, or from the ambient light).*
const THEME: ui::Theme = ui::DAY;
/// Storage a ride takes per hour: a 34-byte record a second (littlefs overhead left out).
const RIDE_BYTES_PER_HOUR: u32 = 34 * 3600;
const BLOCK_SIZE: u32 = <storage::Partition<'_> as littlefs2::driver::Storage>::BLOCK_SIZE as u32;

/// What the screens show (crates/ui).
fn view_of(s: &Status, ride: RideInfo) -> ui::View {
    let fix = s.fix;
    ui::View {
        clock: now_unix(s).map(|unix| {
            let day_s = (unix as i64 + UTC_OFFSET_S as i64).rem_euclid(86_400) as u32;
            ((day_s / 3600) as u8, (day_s / 60 % 60) as u8)
        }),
        battery_pct: s.battery.map(|(_, pct)| pct),
        gnss: match fix {
            None => ui::Gnss::Off,
            Some(f) if f.valid => ui::Gnss::Fix { used: f.sats_used },
            Some(f) => ui::Gnss::Searching {
                in_view: f.sats_in_view,
            },
        },
        // the receiver reports a few tenths of m/s at rest
        speed_mm_s: fix
            .filter(|f| f.valid)
            .and_then(|f| f.speed_mm_s)
            .map(|v| if v < recorder::MOVING_MM_S { 0 } else { v }),
        altitude_dm: s.altimeter.altitude_dm(),
        heart_rate: None,
        ride: ui::RideView {
            timer_s: ride.timer_s,
            distance_cm: ride.distance_cm,
            ascent_m: ride.ascent_m,
            descent_m: ride.descent_m,
            max_speed_mm_s: ride.max_speed_mm_s,
        },
        rides_stored: s.storage.and_then(|r| r.ok()).map(|(rides, _, _)| rides),
        free_hours: s
            .storage
            .and_then(|r| r.ok())
            .map(|(_, free, _)| free as u32 * BLOCK_SIZE / RIDE_BYTES_PER_HOUR),
    }
}

/// Test hook, lines on the USB serial port: `key <0-2> <click|long>` act as key presses from the nRF (`long` =
/// long press then release, as the nRF sends them); `sound <on|off>` turns the buzzer on or off until the next
/// boot. An optional leading sequence number gets an `usb ack <seq>` log line: this link loses ~5 % of the
/// received lines when the log is busy (and log lines too), so the host resends until acknowledged
/// (`tests/hil/dut.py`); a repeated number is acknowledged again but not run twice.
#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: line buffer, statically allocated by embassy"
)]
async fn usb_keys(mut rx: UsbSerialJtagRx<'static, esp_hal::Blocking>) {
    let mut line = [0u8; 32];
    let mut len = 0;
    let mut last_seq = None;
    loop {
        let mut buf = [0u8; 64];
        let n = rx.drain_rx_fifo(&mut buf);
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                let text = core::str::from_utf8(&line[..len]).unwrap_or("");
                let (seq, text) = split_seq(text);
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
                match usb_command(text) {
                    Some(UsbCommand::Key(key, actions)) => {
                        info!("usb: key {key} {actions:?}");
                        for &action in actions {
                            NRF_KEYS.send((key, action)).await;
                        }
                    }
                    Some(UsbCommand::Sound(on)) => {
                        info!("usb: sound {}", if on { "on" } else { "off" });
                        SOUND_ON.store(on, Ordering::Relaxed);
                    }
                    Some(UsbCommand::Gnss(request)) => GNSS_REQUESTS.send(request).await,
                    Some(UsbCommand::RtcShift(s)) => match STATUS.lock(Cell::get).rtc {
                        Some(rtc) => {
                            let t = RtcTime::from_unix(rtc.unix() + s as i64);
                            info!("usb: rtc {rtc:?} -> {t:?} (shift {s} s)");
                            NRF_COMMANDS.send(command::set_rtc(&t, t.weekday())).await;
                        }
                        None => warn!("usb: rtc shift, no RTC time yet"),
                    },
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

/// `<seq> <command>` → (seq, command); a line without a number is a command typed by hand.
fn split_seq(line: &str) -> (Option<u32>, &str) {
    let line = line.trim();
    match line.split_once(' ') {
        Some((n, rest)) if n.bytes().all(|b| b.is_ascii_digit()) => (n.parse().ok(), rest),
        _ => (None, line),
    }
}

enum UsbCommand {
    Key(u8, &'static [KeyAction]),
    Sound(bool),
    Gnss(GnssRequest),
    /// Moves the nRF RTC by this many seconds, to check that a fix sets it right again.
    RtcShift(i32),
}

fn usb_command(line: &str) -> Option<UsbCommand> {
    let mut words = line.split_whitespace();
    match (words.next()?, words.next()?, words.next()) {
        ("sound", "on", None) => Some(UsbCommand::Sound(true)),
        ("sound", "off", None) => Some(UsbCommand::Sound(false)),
        ("gnss", "time", Some(on @ ("on" | "off"))) => {
            Some(UsbCommand::Gnss(GnssRequest::TimeAiding(on == "on")))
        }
        ("gnss", start, None) => {
            let start = match start {
                "hot" => gnss::Start::Hot,
                "warm" => gnss::Start::Warm,
                "cold" => gnss::Start::Cold,
                "full" => gnss::Start::FullCold,
                _ => return None,
            };
            Some(UsbCommand::Gnss(GnssRequest::Start(start)))
        }
        ("rtc", "shift", Some(s)) => Some(UsbCommand::RtcShift(s.parse().ok()?)),
        ("key", key, Some(action)) => {
            let key = key.parse().ok().filter(|&k: &u8| k <= 2)?;
            let actions: &[KeyAction] = match action {
                "click" => &[KeyAction::Click],
                "long" => &[KeyAction::LongPress, KeyAction::LongRelease],
                _ => return None,
            };
            Some(UsbCommand::Key(key, actions))
        }
        _ => None,
    }
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
    let label = MonoTextStyle::new(&FONT_6X10, THEME.mute);
    let value = MonoTextStyle::new(&FONT_10X20, THEME.fg);

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
        let y = ui::CONTENT.top_left.y + 10 + i as i32 * 28;
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
