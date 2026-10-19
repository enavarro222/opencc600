//! Altitude from the barometer, its bias estimated slowly from the GNSS altitude, hardware independent.
//!
//! [`Altimeter`] is a 3-state Kalman filter (altitude, vertical speed, barometer bias): the barometer gives the
//! shape of the profile, the GNSS its level. The model, the tuning and the references are in `docs/ALTITUDE.md`.
//! The elevation gain is not computed here: it comes from the pressure alone ([`pressure_altitude_dm`]), so that
//! the bias corrections never count as climbing.
//!
//! f64 throughout: the ESP32-S3 computes it in software, which costs nothing at 5 Hz, and it avoids the
//! cancellation between a 300 m initial bias uncertainty and a 0.3 m sensor.

#![no_std]

use libm::{pow, round, sqrt};

/// Standard atmosphere sea-level pressure, Pa.
pub const P0_PA: f64 = 101_325.0;
/// T0 / L: 288.15 K / 0.0065 K/m.
const H_SCALE_M: f64 = 44_330.77;
/// R·L / (g·M), the troposphere exponent of the standard atmosphere.
const EXPONENT: f64 = 0.190_263;

/// Pressure altitude (standard atmosphere, troposphere), metres.
pub fn pressure_altitude_m(pressure_pa: f64) -> f64 {
    H_SCALE_M * (1.0 - pow(pressure_pa / P0_PA, EXPONENT))
}

/// Pressure at a pressure altitude, Pa: the inverse of [`pressure_altitude_m`].
pub fn pressure_at_m(altitude_m: f64) -> f64 {
    P0_PA * pow(1.0 - altitude_m / H_SCALE_M, 1.0 / EXPONENT)
}

/// Pressure altitude of a FIT `absolute_pressure` (Pa), decimetres. Software floating point only: the same
/// result on the device and on a PC, so a ride replayed from its file gets the same totals.
pub fn pressure_altitude_dm(pressure_pa: u32) -> i32 {
    round(pressure_altitude_m(pressure_pa as f64) * 10.0) as i32
}

/// Fewer satellites used than this: the GNSS altitude is not used.
pub const MIN_SATS: u8 = 6;
/// HDOP above this: the GNSS altitude is not used.
pub const MAX_HDOP: f64 = 4.0;
/// VDOP is typically 1.5–2 × HDOP, and the user range error ~4 m; the receiver reports neither VDOP nor a
/// vertical accuracy in its trimmed NMEA output.
const VERTICAL_PER_HDOP_M: f64 = 1.7 * 4.0;
const MIN_GNSS_SIGMA_M: f64 = 5.0;

/// Vertical 1σ of a GNSS altitude from the fix quality, metres; None when it is not worth using.
pub fn gnss_sigma_m(hdop: f64, sats_used: u8) -> Option<f64> {
    (sats_used >= MIN_SATS && hdop > 0.0 && hdop <= MAX_HDOP)
        .then(|| (VERTICAL_PER_HDOP_M * hdop).max(MIN_GNSS_SIGMA_M))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Vertical acceleration, 1σ, m/s² (white, piecewise constant over a step). MTB: a 2 m drop in 0.4 s, a
    /// steep descent starting at once; larger follows them closer, with a noisier vertical speed.
    pub accel_sigma: f64,
    /// Barometer measurement noise, m: the sensor gives 0.07 m, wind on the vent, hands and the dynamic pressure
    /// add to it.
    pub baro_sigma_m: f64,
    /// Bias random walk, m/√s (the weather).
    pub bias_walk: f64,
    /// GNSS altitude errors last minutes, the filter takes the 1 Hz samples as independent: their σ is multiplied
    /// by this in the update (not in the gate).
    pub gnss_inflation: f64,
    /// A GNSS altitude further than this many σ from the prediction is rejected.
    pub gate: f64,
    /// After this many GNSS altitudes rejected in a row, the altitude is reset on the GNSS.
    pub reset_after: u16,
    /// Bias 1σ at the start, m (standard atmosphere against the real sea-level pressure).
    pub init_bias_sigma_m: f64,
}

impl Tuning {
    pub const DEFAULT: Self = Self {
        accel_sigma: 1.0,
        baro_sigma_m: 0.3,
        bias_walk: 4.0 / 60.0,
        gnss_inflation: 3.0,
        gate: 3.0,
        reset_after: 60,
        init_bias_sigma_m: 300.0,
    };
}

impl Default for Tuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Longest prediction step, s: after a gap in the barometer stream, the barometer catches up at once anyway.
const MAX_DT_S: f64 = 5.0;
/// Vertical speed 1σ at the start, m/s.
const INIT_VZ_SIGMA: f64 = 2.0;

/// What [`Altimeter::gnss`] did with a GNSS altitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gnss {
    /// No barometer sample yet: nothing to calibrate.
    NoBaro,
    Used,
    /// Outside the gate (a GNSS jump).
    Rejected,
    /// Rejected too long: the altitude was put on the GNSS, the barometric shape kept.
    Reset,
}

const H: usize = 0;
const VZ: usize = 1;
const B: usize = 2;

/// Barometric altimeter, its bias calibrated on the GNSS altitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Altimeter {
    tuning: Tuning,
    /// Altitude (m, MSL), vertical speed (m/s), barometer bias (m): pressure altitude = altitude + bias.
    x: [f64; 3],
    p: [[f64; 3]; 3],
    started: bool,
    /// An altitude reference was used (GNSS or [`Altimeter::set_altitude`]).
    referenced: bool,
    rejected: u16,
}

impl Default for Altimeter {
    fn default() -> Self {
        Self::new(Tuning::DEFAULT)
    }
}

impl Altimeter {
    pub const fn new(tuning: Tuning) -> Self {
        Self {
            tuning,
            x: [0.0; 3],
            p: [[0.0; 3]; 3],
            started: false,
            referenced: false,
            rejected: 0,
        }
    }

    /// A barometer sample, `dt_s` after the previous one.
    pub fn baro(&mut self, dt_s: f64, pressure_pa: f64) {
        let z = pressure_altitude_m(pressure_pa);
        if !self.started {
            let s2 = self.tuning.init_bias_sigma_m * self.tuning.init_bias_sigma_m;
            self.x = [z, 0.0, 0.0];
            // h + b = z is known to the sensor noise, h and b separately to the initial bias uncertainty.
            self.p = [
                [s2, 0.0, -s2],
                [0.0, INIT_VZ_SIGMA * INIT_VZ_SIGMA, 0.0],
                [
                    -s2,
                    0.0,
                    s2 + self.tuning.baro_sigma_m * self.tuning.baro_sigma_m,
                ],
            ];
            self.started = true;
            return;
        }
        self.predict(dt_s.clamp(0.0, MAX_DT_S));
        let r = self.tuning.baro_sigma_m * self.tuning.baro_sigma_m;
        self.update([1.0, 0.0, 1.0], z, r);
    }

    /// A GNSS altitude (m, MSL) and its vertical 1σ ([`gnss_sigma_m`]).
    pub fn gnss(&mut self, altitude_m: f64, sigma_m: f64) -> Gnss {
        if !self.started {
            return Gnss::NoBaro;
        }
        let innovation = altitude_m - self.x[H];
        let gate = self.tuning.gate * sqrt(self.p[H][H] + sigma_m * sigma_m);
        if innovation.abs() > gate {
            self.rejected += 1;
            if self.rejected < self.tuning.reset_after {
                return Gnss::Rejected;
            }
            self.set_altitude(altitude_m, sigma_m * self.tuning.gnss_inflation);
            return Gnss::Reset;
        }
        self.rejected = 0;
        let s = sigma_m * self.tuning.gnss_inflation;
        self.update([1.0, 0.0, 0.0], altitude_m, s * s);
        self.referenced = true;
        Gnss::Used
    }

    /// A known altitude (m) with its 1σ: puts the altitude there and keeps the barometric shape (`h + b`).
    /// Ignored before the first barometer sample.
    pub fn set_altitude(&mut self, altitude_m: f64, sigma_m: f64) {
        if !self.started {
            return;
        }
        let p = &self.p;
        let baro = self.x[H] + self.x[B];
        let baro_var = p[H][H] + 2.0 * p[H][B] + p[B][B];
        let baro_vz = p[H][VZ] + p[B][VZ];
        let s2 = sigma_m * sigma_m;
        self.x[H] = altitude_m;
        self.x[B] = baro - altitude_m;
        let vz = p[VZ][VZ];
        self.p = [
            [s2, 0.0, -s2],
            [0.0, vz, baro_vz],
            [-s2, baro_vz, s2 + baro_var],
        ];
        self.rejected = 0;
        self.referenced = true;
    }

    /// Altitude, m (MSL); None before the first barometer sample and the first altitude reference.
    pub fn altitude_m(&self) -> Option<f64> {
        (self.started && self.referenced).then_some(self.x[H])
    }

    /// [`Altimeter::altitude_m`] in decimetres, rounded (FIT records).
    pub fn altitude_dm(&self) -> Option<i32> {
        self.altitude_m().map(|h| round(h * 10.0) as i32)
    }

    /// Altitude 1σ, m.
    pub fn altitude_sigma_m(&self) -> f64 {
        sqrt(self.p[H][H])
    }

    /// Vertical speed, m/s (positive up).
    pub fn vertical_speed_m_s(&self) -> f64 {
        self.x[VZ]
    }

    /// Barometer bias, m: pressure altitude minus altitude.
    pub fn bias_m(&self) -> f64 {
        self.x[B]
    }

    fn predict(&mut self, dt: f64) {
        let p = self.p;
        // P ← F P Fᵀ, F = [[1, dt, 0], [0, 1, 0], [0, 0, 1]]
        let mut n = p;
        n[H][H] = p[H][H] + dt * (p[VZ][H] + p[H][VZ]) + dt * dt * p[VZ][VZ];
        n[H][VZ] = p[H][VZ] + dt * p[VZ][VZ];
        n[H][B] = p[H][B] + dt * p[VZ][B];
        n[VZ][H] = n[H][VZ];
        n[B][H] = n[H][B];
        // Q: white acceleration, piecewise constant; bias random walk
        let a2 = self.tuning.accel_sigma * self.tuning.accel_sigma;
        n[H][H] += a2 * dt * dt * dt * dt / 4.0;
        n[H][VZ] += a2 * dt * dt * dt / 2.0;
        n[VZ][H] += a2 * dt * dt * dt / 2.0;
        n[VZ][VZ] += a2 * dt * dt;
        n[B][B] += self.tuning.bias_walk * self.tuning.bias_walk * dt;
        self.p = n;
        self.x[H] += dt * self.x[VZ];
    }

    /// Scalar measurement `z = H·x` with variance `r`.
    fn update(&mut self, h: [f64; 3], z: f64, r: f64) {
        let mut ph = [0.0; 3];
        for (i, v) in ph.iter_mut().enumerate() {
            *v = (0..3).map(|j| self.p[i][j] * h[j]).sum();
        }
        let s: f64 = (0..3).map(|i| h[i] * ph[i]).sum::<f64>() + r;
        let innovation = z - (0..3).map(|i| h[i] * self.x[i]).sum::<f64>();
        for i in 0..3 {
            self.x[i] += ph[i] / s * innovation;
            for j in 0..3 {
                // P ← P − K S Kᵀ = P − (P Hᵀ)(P Hᵀ)ᵀ / S: symmetric by construction
                self.p[i][j] -= ph[i] * ph[j] / s;
            }
        }
    }
}
