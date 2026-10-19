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

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Text};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::dma_tx_buffer;
use esp_hal::gpio::{DriveMode, Level, Output, OutputConfig};
use esp_hal::lcd_cam::LcdCam;
use esp_hal::lcd_cam::lcd::i8080::{self, I8080};
use esp_hal::ledc::channel::{self, ChannelIFace};
use esp_hal::ledc::timer::{self, TimerIFace};
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed};
use esp_hal::time::{self as hal_time, Rate};
use esp_hal::timer::timg::TimerGroup;
use log::info;
use opencc600_firmware::panel::{self, FrameBuffer, Panel};

extern crate alloc;

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
async fn main(_spawner: Spawner) -> ! {
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

    info!("opencc600 spike: booted from ota_0");
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

    bench(&mut panel, &mut fb);
    draw_test_pattern(&mut fb);
    panel.flush(&fb);

    let mut ticks: u32 = 0;
    loop {
        info!("alive {ticks}");
        ticks += 1;
        Timer::after(Duration::from_secs(1)).await;
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
