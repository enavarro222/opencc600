//! The screen and the light: the backlight from the ambient light sensor (`auto_backlight` setting) and the night
//! theme from sunset to sunrise (`auto_night`).

use core::f64::consts::PI;

/// Raw ambient light (nRF stream `F1 04`, tag 600) at and below which the backlight is at [`MIN_PCT`]: ~1000
/// indoors at night, 411 covered.
pub const DARK: u16 = 600;
/// Raw ambient light at and above which the backlight is at [`MAX_PCT`]: ~2500 indoors by day, the sensor seems to
/// saturate near 2600.
pub const BRIGHT: u16 = 2400;
pub const MIN_PCT: u8 = 10;
pub const MAX_PCT: u8 = 100;
/// Time constant of the light's average: a shadow or a passing car's lights hardly change the backlight.
const AVERAGE_MS: f64 = 3_000.0;
/// The backlight changes by this much at least (or to its bounds), so the sensor's noise does not make it flicker.
const STEP_PCT: u8 = 5;

/// The backlight from the ambient light, averaged.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoBacklight {
    level: Option<(f64, u64)>,
    pct: Option<u8>,
}

impl AutoBacklight {
    pub const fn new() -> Self {
        Self { level: None, pct: None }
    }

    /// Takes the latest raw level at `now_ms`; the backlight to set, percent, when it changed enough.
    pub fn update(&mut self, raw: u16, now_ms: u64) -> Option<u8> {
        let level = match self.level {
            None => raw as f64,
            Some((avg, at)) => {
                let dt = now_ms.saturating_sub(at) as f64;
                avg + (raw as f64 - avg) * dt / (AVERAGE_MS + dt)
            }
        };
        self.level = Some((level, now_ms));
        let target = pct_for(level);
        let changed = match self.pct {
            None => true,
            Some(pct) => pct.abs_diff(target) >= STEP_PCT || (target != pct && matches!(target, MIN_PCT | MAX_PCT)),
        };
        changed.then(|| {
            self.pct = Some(target);
            target
        })
    }
}

/// The backlight for a raw ambient light level: [`MIN_PCT`] to [`MAX_PCT`], linear from [`DARK`] to [`BRIGHT`].
fn pct_for(level: f64) -> u8 {
    let x = ((level - DARK as f64) / (BRIGHT - DARK) as f64).clamp(0.0, 1.0);
    libm::round(MIN_PCT as f64 + x * (MAX_PCT - MIN_PCT) as f64) as u8
}

/// The sun's elevation above the horizon, degrees, at Unix time `unix` and a position (1e-7 degrees). Low-precision
/// formulas of the Astronomical Almanac: ~0.01° from 1950 to 2050.
pub fn sun_elevation_deg(unix: i64, lat_e7: i32, lon_e7: i32) -> f64 {
    let rad = PI / 180.0;
    // days since J2000.0 (2000-01-01 12:00 UTC)
    let d = unix as f64 / 86_400.0 - 10_957.5;
    let g = (357.529 + 0.985_600_28 * d) * rad;
    let q = 280.459 + 0.985_647_36 * d;
    let ecliptic = (q + 1.915 * libm::sin(g) + 0.020 * libm::sin(2.0 * g)) * rad;
    let obliquity = (23.439 - 0.000_000_36 * d) * rad;
    let right_ascension = libm::atan2(libm::cos(obliquity) * libm::sin(ecliptic), libm::cos(ecliptic));
    let declination = libm::asin(libm::sin(obliquity) * libm::sin(ecliptic));
    let sidereal = (280.460_618_37 + 360.985_647_366_29 * d) * rad;
    let hour_angle = sidereal + lon_e7 as f64 * 1e-7 * rad - right_ascension;
    let lat = lat_e7 as f64 * 1e-7 * rad;
    let sin_elevation =
        libm::sin(lat) * libm::sin(declination) + libm::cos(lat) * libm::cos(declination) * libm::cos(hour_angle);
    libm::asin(sin_elevation.clamp(-1.0, 1.0)) / rad
}

/// Between sunset and sunrise: the sun's centre more than 0.833° below the horizon (its radius and the refraction,
/// as the published times count them).
pub fn is_night(unix: i64, lat_e7: i32, lon_e7: i32) -> bool {
    sun_elevation_deg(unix, lat_e7, lon_e7) < -0.833
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-06-21 and 2026-12-21, 00:00 UTC
    const JUNE_21: i64 = 1_782_000_000;
    const DEC_21: i64 = 1_797_811_200;
    const PARIS: (i32, i32) = (488_566_000, 23_522_000);

    fn night(unix: i64, (lat, lon): (i32, i32)) -> bool {
        is_night(unix, lat, lon)
    }

    #[test]
    fn sunset_and_sunrise_in_paris() {
        let h = |day: i64, h: i64, m: i64| day + h * 3600 + m * 60;
        // 21 June: sunrise 03:47 UTC, sunset 19:58 UTC
        assert!(night(h(JUNE_21, 3, 40), PARIS) && !night(h(JUNE_21, 3, 55), PARIS));
        assert!(!night(h(JUNE_21, 19, 50), PARIS) && night(h(JUNE_21, 20, 5), PARIS));
        // 21 December: sunrise 07:41 UTC, sunset 15:56 UTC
        assert!(night(h(DEC_21, 7, 35), PARIS) && !night(h(DEC_21, 7, 50), PARIS));
        assert!(!night(h(DEC_21, 15, 50), PARIS) && night(h(DEC_21, 16, 5), PARIS));
        // noon: 64.6° in June, 17.7° in December
        assert!((sun_elevation_deg(h(JUNE_21, 11, 51), PARIS.0, PARIS.1) - 64.6).abs() < 0.2);
        assert!((sun_elevation_deg(h(DEC_21, 11, 49), PARIS.0, PARIS.1) - 17.7).abs() < 0.2);
    }

    #[test]
    fn polar_day_and_night() {
        let tromso = (696_492_000, 189_553_000);
        assert!((0..24).all(|hour| !night(JUNE_21 + hour * 3600, tromso)));
        assert!((0..24).all(|hour| night(DEC_21 + hour * 3600, tromso)));
    }

    #[test]
    fn backlight_follows_the_light_without_flicker() {
        let mut b = AutoBacklight::new();
        assert_eq!(b.update(400, 0), Some(MIN_PCT), "the first level at once");
        assert_eq!(b.update(DARK, 300), None);
        // daylight: up to the top within a few seconds
        let mut pct = MIN_PCT;
        for t in 1..=40 {
            if let Some(p) = b.update(2600, 300 * t) {
                assert!(p > pct);
                pct = p;
            }
        }
        assert_eq!(pct, MAX_PCT);
        // a shadow for half a second: hardly a change
        for t in [12_300, 12_600] {
            assert!(b.update(DARK, t).is_none_or(|p| p >= 85));
        }
        // noise around a level: one change, then none
        let mut b = AutoBacklight::new();
        let changes = (0..100)
            .filter(|&t| b.update(1500 + (t % 3) as u16 * 20, 300 * t).is_some())
            .count();
        assert_eq!(changes, 1);
    }
}
