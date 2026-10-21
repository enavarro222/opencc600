//! Cycling sensors, hardware independent: the BLE GATT measurements decoded ([`ble`]), the ANT+ pages decoded and
//! the channels kept open ([`ant`]), and the rates computed from
//! cumulative revolution counts ([`Revs`]), as the speed / cadence sensors and power meters send them.

#![no_std]

pub mod ant;
pub mod ble;

/// What a power meter answered to a zero offset calibration (cranks still, no weight on the pedals).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calibration {
    /// Done: the offset found, in the meter's own units (to compare from one calibration to the next).
    Done(i16),
    Failed,
    /// The meter cannot do it.
    Unsupported,
}

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

/// The average of the last [`Avg3s::SECONDS`] seconds of a value (3 s power): one bucket a second, each the average
/// of the values that came in it. `None` once nothing came for that long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Avg3s {
    /// Second (of `now_ms`), sum, count.
    buckets: [Option<(u64, u32, u32)>; Self::SECONDS],
}

impl Avg3s {
    pub const SECONDS: usize = 3;

    pub const fn new() -> Self {
        Self { buckets: [None; Self::SECONDS] }
    }

    /// A value at `now_ms` (any monotonic clock); `None` (no value now) only ages the buckets.
    pub fn push(&mut self, value: Option<u16>, now_ms: u64) {
        let second = now_ms / 1000;
        let Some(v) = value else { return };
        let slot = &mut self.buckets[(second % Self::SECONDS as u64) as usize];
        match slot {
            Some((s, sum, n)) if *s == second => {
                *sum += v as u32;
                *n += 1;
            }
            _ => *slot = Some((second, v as u32, 1)),
        }
    }

    /// The average now, of the buckets of the last [`Self::SECONDS`] seconds.
    pub fn get(&self, now_ms: u64) -> Option<u16> {
        let second = now_ms / 1000;
        let (sum, n) = self
            .buckets
            .iter()
            .flatten()
            .filter(|(s, _, _)| second.saturating_sub(*s) < Self::SECONDS as u64)
            .fold((0u32, 0u32), |(sum, n), (_, bs, bn)| (sum + bs / bn, n + 1));
        (n > 0).then(|| ((sum + n / 2) / n) as u16)
    }
}
