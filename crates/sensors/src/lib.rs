//! Cycling sensors, hardware independent: the BLE GATT measurements decoded ([`ble`]), the ANT+ pages decoded and
//! the channels kept open ([`ant`]), and the rates computed from
//! cumulative revolution counts ([`Revs`]), as the speed / cadence sensors and power meters send them.

#![no_std]

pub mod ant;
pub mod ble;

/// A revolution rate from a cumulative revolution count and the time of its last event (1/1024 s), as the
/// speed / cadence sensors send them (both counters wrap). The rate holds while the sensor repeats the same event
/// (it sends ~4 a second, a revolution may take longer), and drops to 0 after [`Revs::STOP_MS`] without a new one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Revs {
    last: Option<Event>,
    /// Revolutions per 1000 minutes (cadence rpm × 1000); `None` until two events give one.
    rate: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event {
    revs: u32,
    time: u16,
    at_ms: u64,
}

impl Revs {
    /// No new revolution for this long: stopped.
    pub const STOP_MS: u64 = 3_000;
    /// More than this many revolutions between two measurements: a reset or a lost sensor, not a rate.
    const MAX_STEP: u32 = 64;

    /// Takes a measurement: the cumulative count (`mask`: the counter's width, `u16::MAX` for a crank, `u32::MAX`
    /// for a wheel), the event time, and when it came (`now_ms`, any monotonic clock). Returns the new revolutions.
    pub fn update(&mut self, revs: u32, time: u16, mask: u32, now_ms: u64) -> u32 {
        let Some(last) = self.last else {
            self.last = Some(Event { revs, time, at_ms: now_ms });
            return 0;
        };
        let n = revs.wrapping_sub(last.revs) & mask;
        let dt = time.wrapping_sub(last.time);
        if n > Self::MAX_STEP {
            self.last = Some(Event { revs, time, at_ms: now_ms });
            self.rate = None;
            return 0;
        }
        if n == 0 || dt == 0 {
            // the same event again
            if now_ms.saturating_sub(last.at_ms) > Self::STOP_MS {
                self.rate = Some(0);
            }
            return 0;
        }
        self.rate = Some((n as u64 * 1024 * 60_000 / dt as u64) as u32);
        self.last = Some(Event { revs, time, at_ms: now_ms });
        n
    }

    /// Revolutions per minute (rounded), 0 when stopped; `None` before a rate is known (the first event, after a
    /// reset of the sensor's counter).
    pub fn rpm(&self) -> Option<u16> {
        self.rate.map(|r| ((r + 500) / 1000).min(u16::MAX as u32) as u16)
    }

    /// The speed for a wheel of `circumference_mm`, mm/s.
    pub fn speed_mm_s(&self, circumference_mm: u16) -> Option<u32> {
        self.rate.map(|r| (r as u64 * circumference_mm as u64 / 60_000) as u32)
    }

    /// Forgets the last event (the sensor was lost).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
