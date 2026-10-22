//! The GNSS receiver on UART0 (crates/gnss): its fixes into the status, its output setup, the time aiding, the
//! power-ons while it is silent, the RTC set from it.

use core::cell::Cell;
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use log::{debug, info, warn};
use nrf_link::{RtcTime, command};
use opencc600_firmware::gnss_uart::{self, GnssUart};
use status::GnssLink;

use alloc::format;

use crate::nrf::NRF_COMMANDS;
use crate::{STATUS, STATUS_CHANGED};

/// When the last position came over USB (`fix`, tools/replay_gpx.py): until [`SIM_FIX_HOLD`] after it, the
/// receiver's epochs are ignored.
pub(crate) static SIM_FIX_AT: Mutex<CriticalSectionRawMutex, Cell<Option<Instant>>> = Mutex::new(Cell::new(None));

pub(crate) const SIM_FIX_HOLD: Duration = Duration::from_secs(5);

pub(crate) fn sim_fix_active() -> bool {
    SIM_FIX_AT.lock(Cell::get).is_some_and(|t| t.elapsed() < SIM_FIX_HOLD)
}

/// Requests to the GNSS task from the USB test hook (time-to-first-fix measurements).
pub(crate) enum GnssRequest {
    Start(gnss::Start),
    /// Answer the receiver's time requests (and each restart) with the RTC time (`$PAIR590`).
    TimeAiding(bool),
}

pub(crate) static GNSS_REQUESTS: Channel<CriticalSectionRawMutex, GnssRequest, 4> = Channel::new();

#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "task future: line buffer + fix, statically allocated by embassy"
)]
pub(crate) async fn gnss_task(mut uart: GnssUart) {
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

pub(crate) fn send_pair(uart: &mut GnssUart, body: &str) {
    let mut out = [0; gnss_uart::MAX_COMMAND];
    match gnss::write_command(body, &mut out) {
        Some(len) => uart.send(&out[..len]),
        None => warn!("gnss: command too long: {body}"),
    }
}

/// Calibrates the barometer on the GNSS altitude of a valid fix good enough for it.
pub(crate) fn calibrate(altimeter: &mut altitude::Altimeter, fix: &gnss::Fix) -> Option<altitude::Gnss> {
    let (true, Some(alt_dm), Some(hdop_c)) = (fix.valid, fix.alt_dm, fix.hdop_c) else {
        return None;
    };
    let sigma = altitude::gnss_sigma_m(hdop_c as f64 / 100.0, fix.sats_used)?;
    Some(altimeter.gnss(alt_dm as f64 / 10.0, sigma))
}

/// Writes the GNSS time to the nRF RTC when they differ by 2 s or more (the RTC frame has a 1 s resolution and
/// arrives at 5 Hz, so 1 s is jitter), at most once a minute in case a write does not take.
pub(crate) async fn sync_rtc(fix: &gnss::Fix, last_set: &mut Option<Instant>) {
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

/// Test hook (`gnss off N`): the receiver power-ons to drop.
pub(crate) static GNSS_POWER_LOST: AtomicU8 = AtomicU8::new(0);
