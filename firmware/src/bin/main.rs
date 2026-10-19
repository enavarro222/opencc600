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
use esp_rtos::embassy::InterruptExecutor;
use esp_storage::FlashStorage;
use littlefs2::fs::{Allocation, Filesystem};
use log::{debug, info, warn};
use nrf_link::{Decoder, KeyAction, Message, RtcTime, command};
use opencc600_firmware::gnss_uart::GnssUart;
#[cfg(feature = "hil-storage")]
use opencc600_firmware::hil_storage;
use opencc600_firmware::panel::{self, FrameBuffer, Panel};
use opencc600_firmware::storage;
use static_cell::StaticCell;

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

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
    let mut probe_index: Option<usize> = None;
    let mut scan: Option<(u8, Instant)> = None;
    let mut power_prompt: Option<Instant> = None;
    loop {
        let _ = with_timeout(Duration::from_secs(1), STATUS_CHANGED.wait()).await;
        // The vendor repeats SendPowerOnCmd; c606-oss does it every 5 s. No nRF watchdog seen so far.
        if last_power_on.elapsed() >= Duration::from_secs(5) {
            NRF_COMMANDS.send(command::power_on()).await;
            last_power_on = Instant::now();
        }
        // Buzzer hunt: key 1 = next probe, key 2 = send the same again, key 0 = previous;
        // long press on key 1 = automatic scan of E2 01 00..1B, one every 3 s (tools/sound_probe.py records it).
        let mut send = None;
        let mut scan_done = false;
        while let Ok((key, action)) = NRF_KEYS.try_receive() {
            // Power off like the vendor: long press on key 0 asks, a click on key 0 confirms, any other click or
            // 8 s cancels. The nRF repeats LongPress while the key is held: only the first one opens the prompt.
            if power_prompt.is_some() {
                match (key, action) {
                    (0, KeyAction::Click) => {
                        info!("power off");
                        draw_message(&mut fb, "Power off");
                        panel.flush(&fb);
                        NRF_COMMANDS.send(command::power_off()).await;
                        Timer::after(Duration::from_secs(2)).await;
                        warn!("still powered 2 s after power_off");
                        power_prompt = None;
                    }
                    (_, KeyAction::Click) => power_prompt = None,
                    _ => {}
                }
                continue;
            }
            match (key, action) {
                (0, KeyAction::LongPress) => power_prompt = Some(Instant::now()),
                (1, KeyAction::LongRelease) if scan.is_none() => {
                    info!("scan start");
                    scan = Some((0u8, Instant::now() - SCAN_STEP));
                }
                (_, KeyAction::Click) => {
                    probe_index = match key {
                        0 => probe_index.map(|i: usize| i.saturating_sub(1)),
                        1 => Some(probe_index.map_or(0, |i| (i + 1).min(PROBES.len() - 1))),
                        _ => probe_index.or(Some(0)),
                    };
                    send = Some(PROBES[probe_index.unwrap_or(0)]);
                }
                _ => {}
            }
        }
        if let Some((id, at)) = scan
            && at.elapsed() >= SCAN_STEP
        {
            send = Some(id);
            scan = (id < SCAN_LAST).then(|| (id + 1, Instant::now()));
            scan_done = scan.is_none();
        }
        if let Some(id) = send {
            info!("probe E2 01 {id:02X}");
            NRF_COMMANDS.send(command::misc(id)).await;
            STATUS.lock(|s| {
                let mut status = s.get();
                status.probe = Some(id);
                s.set(status);
            });
            if scan_done {
                info!("scan done");
            }
        }
        if power_prompt.is_some_and(|at| at.elapsed() >= Duration::from_secs(8)) {
            power_prompt = None;
        }
        let state = STATUS.lock(Cell::get);
        let screen = (state, power_prompt.is_some());
        if shown != Some(screen) {
            draw_status(&mut fb, &state);
            if power_prompt.is_some() {
                draw_message(&mut fb, "Power off?\nkey 0: yes\nother: no");
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

const SCAN_STEP: Duration = Duration::from_secs(3);
const SCAN_LAST: u8 = 0x1B;

/// Buzzer hunt: `E2 01 XX` values, the ones the vendor sends first, then the rest of 0..0x1C.
const PROBES: [u8; 28] = [
    0x13, 0x17, 0x18, 0x19, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
    0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x14, 0x15, 0x16, 0x1A, 0x1B,
];
static NRF_COMMANDS: Channel<CriticalSectionRawMutex, [u8; command::CTRL_FRAME], 4> =
    Channel::new();

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
                Message::Battery { .. } | Message::Status(_) | Message::Rtc(_) => {
                    debug!("nrf {msg:?}")
                }
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
async fn gnss_task(mut uart: GnssUart) {
    let mut reader = gnss::LineReader::new();
    let mut fix = gnss::Fix::default();
    let mut last_report = Instant::now();
    let mut stalled = false;
    loop {
        let mut epoch = false;
        uart.drain(|data| {
            reader.feed(data, |line| {
                debug!("gnss {line}");
                epoch |= fix.update(line) == gnss::Epoch::Complete;
            })
        });
        if uart.stalled() && !stalled {
            stalled = true;
            warn!("gnss: DMA stopped, data lost");
        }
        if epoch {
            STATUS.lock(|s| {
                let mut status = s.get();
                status.fix = Some(fix);
                status.gnss = Some(reader.stats);
                s.set(status);
            });
            STATUS_CHANGED.signal(());
        }
        if last_report.elapsed() >= Duration::from_secs(10) {
            last_report = Instant::now();
            info!(
                "gnss: {:?}, fix {} q{} {}/{} sats{}",
                reader.stats,
                fix.valid,
                fix.quality,
                fix.sats_used,
                fix.sats_in_view,
                if stalled { " DMA STOPPED" } else { "" }
            );
        }
        Timer::after(Duration::from_millis(50)).await;
    }
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
    /// Ambient light (raw) and the raw sensor stream subtype `0A` (meaning unknown).
    light: Option<u16>,
    stream0a: Option<[u8; 6]>,
    /// Last `E2 01 XX` probe sent (buzzer hunt).
    probe: Option<u8>,
    /// Storage partition: Ok((finished rides, free blocks, total blocks)) or the littlefs error code.
    storage: Option<Result<(u16, u16, u16), i32>>,
    /// GNSS link counters and latest complete fix.
    gnss: Option<gnss::Stats>,
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
            light: None,
            stream0a: None,
            probe: None,
            storage: None,
            gnss: None,
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
            } => self.baro = Some((centi_celsius, centi_pascal)),
            Message::Light { level, .. } => self.light = Some(level),
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
fn draw_status(fb: &mut FrameBuffer, s: &Status) {
    let title = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let label = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY);
    let value = MonoTextStyle::new(&FONT_10X20, Rgb565::YELLOW);
    fb.clear(Rgb565::BLACK).unwrap();
    Text::with_alignment("opencc600", Point::new(120, 24), title, Alignment::Center)
        .draw(fb)
        .unwrap();

    let none = || String::from("-");
    let rows: [(&str, String); 8] = [
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
            "baro",
            s.baro.map_or_else(
                || String::from("- (long press key 0)"),
                |(t, p)| {
                    format!(
                        "{}.{:02}C {}.{:01}hPa",
                        t / 100,
                        (t % 100).abs(),
                        p / 10_000,
                        p / 1000 % 10
                    )
                },
            ),
        ),
        (
            "ambient light (raw)",
            s.light.map_or_else(none, |l| format!("{l}")),
        ),
    ];
    for (i, (name, v)) in rows.iter().enumerate() {
        let y = 48 + i as i32 * 30;
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
