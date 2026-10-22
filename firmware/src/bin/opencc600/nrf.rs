//! The nRF co-processor on UART2 (crates/nrf-link): its frames read into the status, the commands sent, the
//! keys, the buzzer.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant};
use esp_hal::Async;
use esp_hal::peripherals::{GPIO41, GPIO42, UART2};
use esp_hal::uart::{self, Uart, UartRx, UartTx};
use log::{debug, info, warn};
use nrf_link::{Decoder, KeyAction, Message, command};
use opencc600_firmware::ant;

use alloc::format;

use crate::{STATUS, STATUS_CHANGED};

pub(crate) static NRF_KEYS: Channel<CriticalSectionRawMutex, (u8, KeyAction), 8> = Channel::new();

/// The last barometer frame of the nRF's sensor stream: it only starts on the nRF's own power-on gesture
/// (a long press of the power key), not when the charger starts the device, nor after a power off.
pub(crate) static STREAM_AT: Mutex<CriticalSectionRawMutex, Cell<Option<Instant>>> = Mutex::new(Cell::new(None));

/// The nRF's sensor stream is missing (none for 3 s, past the first 3 s after boot).
pub(crate) fn stream_lost() -> bool {
    let quiet = Duration::from_secs(3);
    Instant::now().duration_since(Instant::from_ticks(0)) >= quiet
        && STREAM_AT.lock(Cell::get).is_none_or(|at| at.elapsed() >= quiet)
}

/// The key events since the last trip log line (its `keys` column), oldest first: key, then `C` click, `L` long
/// press (repeated while held), `R` its release, `?` other; `u` before the ones typed on the USB test hook.
pub(crate) static KEY_LOG: Mutex<CriticalSectionRawMutex, RefCell<heapless::String<64>>> =
    Mutex::new(RefCell::new(heapless::String::new()));

pub(crate) fn log_key(key: u8, action: KeyAction, usb: bool) {
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

pub(crate) static NRF_COMMANDS: Channel<CriticalSectionRawMutex, [u8; command::CTRL_FRAME], 4> =
    Channel::new();

/// UART2, TX GPIO42 / RX GPIO41, 115200 8N1. Built here: the async UART halves are not `Send`, so they cannot be
/// handed over from the thread-mode executor.
#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: decoder + buffers, statically allocated by embassy"
)]
pub(crate) async fn nrf_link(uart: UART2<'static>, tx_pin: GPIO42<'static>, rx_pin: GPIO41<'static>) {
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
pub(crate) async fn nrf_tx(mut tx: UartTx<'static, Async>) {
    loop {
        let frame = NRF_COMMANDS.receive().await;
        if let Err(e) = tx.write_async(&frame).await {
            warn!("nrf uart tx: {e:?}");
        }
    }
}

pub(crate) async fn nrf_rx(mut rx: UartRx<'static, Async>) -> ! {
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

/// Short beep on the buzzer (`E2 01 00`).
pub(crate) const SOUND_BEEP: u8 = 0x00;

/// Buzzer on; `sound off` on the USB test hook mutes it (tests next to people).
pub(crate) static SOUND_ON: AtomicBool = AtomicBool::new(true);

pub(crate) fn sound(id: u8) {
    if SOUND_ON.load(Ordering::Relaxed) {
        let _ = NRF_COMMANDS.try_send(command::misc(id));
    }
}
