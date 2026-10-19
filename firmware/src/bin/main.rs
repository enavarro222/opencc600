//! Step 0 bring-up spike (docs/PRODUCT.md, technical plan).
//!
//! Proves that an esp-hal image written to `ota_0` boots under the vendor second-stage bootloader and
//! partition table, with the 2 MB quad PSRAM. Done when: backlight on + log line on the USB serial.

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
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::DriveMode;
use esp_hal::ledc::channel::{self, ChannelIFace};
use esp_hal::ledc::timer::{self, TimerIFace};
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use log::info;

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
    esp_alloc::psram_allocator!(
        peripherals.PSRAM,
        esp_hal::psram,
        esp_hal::psram::PsramConfig {
            mode: esp_hal::psram::PsramMode::QuadSpi,
            ..Default::default()
        }
    );

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("opencc600 spike: booted from ota_0");
    info!("heap: {}", esp_alloc::HEAP.stats());

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

    let mut ticks: u32 = 0;
    loop {
        info!("alive {ticks}");
        ticks += 1;
        Timer::after(Duration::from_secs(1)).await;
    }
}
