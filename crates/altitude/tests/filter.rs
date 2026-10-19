//! The filter on synthetic rides: barometer at 5 Hz (0.8 Pa noise, as measured on the CC600), GNSS at 1 Hz with
//! white noise plus an error correlated over a minute (seeded, reproducible).

use altitude::{
    Altimeter, Gnss, gnss_sigma_m, pressure_altitude_dm, pressure_altitude_m, pressure_at_m,
};

/// xorshift64*, Box–Muller.
struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (2.0 * std::f64::consts::PI * self.uniform()).cos()
    }
}

const BARO_NOISE_PA: f64 = 0.8;
const GNSS_WHITE_M: f64 = 4.0;
/// First-order Gauss–Markov error, 60 s correlation time.
const GNSS_SLOW_M: f64 = 5.0;
const GNSS_TAU_S: f64 = 60.0;

/// One instant of a synthetic ride.
struct At {
    altitude: f64,
    /// Pressure altitude minus altitude (the weather).
    bias: f64,
    /// Added to the GNSS altitude (a jump); None: no GNSS altitude this second.
    gnss_extra: Option<f64>,
}

fn at(altitude: f64, bias: f64) -> At {
    At {
        altitude,
        bias,
        gnss_extra: Some(0.0),
    }
}

struct Sim {
    alt: Altimeter,
    rng: Rng,
    slow: f64,
    /// Seconds.
    t: u32,
}

impl Sim {
    fn new(seed: u64) -> Self {
        Self {
            alt: Altimeter::default(),
            rng: Rng(seed),
            slow: 0.0,
            t: 0,
        }
    }

    /// Runs `seconds` more, calling `check(t, truth, filter, gnss result)` after each second.
    fn run(
        &mut self,
        seconds: u32,
        ride: impl Fn(u32) -> At,
        mut check: impl FnMut(u32, f64, &Altimeter, Option<Gnss>),
    ) {
        let sigma = gnss_sigma_m(1.0, 12).unwrap();
        let decay = (-1.0 / GNSS_TAU_S).exp();
        for _ in 0..seconds {
            let now = ride(self.t);
            let next = ride(self.t + 1);
            for i in 1..=5 {
                // linear within the second: a staircase would make the vertical speed drop on each step
                let f = i as f64 / 5.0;
                let h = now.altitude + now.bias;
                let h = h + (next.altitude + next.bias - h) * f;
                let p = pressure_at_m(h) + BARO_NOISE_PA * self.rng.gauss();
                self.alt.baro(0.2, p);
            }
            self.slow =
                self.slow * decay + (1.0 - decay * decay).sqrt() * GNSS_SLOW_M * self.rng.gauss();
            let white = GNSS_WHITE_M * self.rng.gauss();
            let result = now.gnss_extra.map(|extra| {
                self.alt
                    .gnss(now.altitude + self.slow + white + extra, sigma)
            });
            check(self.t, now.altitude, &self.alt, result);
            self.t += 1;
        }
    }
}

/// Altitude errors over a run: the GNSS error correlated over a minute leaves a few metres on the level, so the
/// checks are on the mean (a lag), the RMS and the worst case.
#[derive(Default)]
struct Errors {
    n: u32,
    sum: f64,
    sum2: f64,
    worst: f64,
}

impl Errors {
    fn add(&mut self, e: f64) {
        self.n += 1;
        self.sum += e;
        self.sum2 += e * e;
        self.worst = self.worst.max(e.abs());
    }

    fn check(&self, mean: f64, rms: f64, worst: f64) {
        let m = self.sum / self.n as f64;
        let r = (self.sum2 / self.n as f64).sqrt();
        println!("mean {m:.2} m, rms {r:.2} m, worst {:.2} m", self.worst);
        assert!(
            m.abs() < mean && r < rms && self.worst < worst,
            "mean {m:.2} m, rms {r:.2} m, worst {:.2} m",
            self.worst
        );
    }
}

fn error(truth: f64, alt: &Altimeter) -> f64 {
    (alt.altitude_m().unwrap() - truth).abs()
}

#[test]
fn pressure_altitude_is_the_standard_atmosphere() {
    assert_eq!(pressure_at_m(0.0), 101_325.0);
    // US Standard Atmosphere 1976 tables: 89 874.6 Pa at 1000 m, 79 495.2 Pa at 2000 m
    assert!((pressure_at_m(1000.0) - 89_874.6).abs() < 1.0);
    assert!((pressure_at_m(2000.0) - 79_495.2).abs() < 1.0);
    for h in (-400..4000).step_by(100) {
        assert!((pressure_altitude_m(pressure_at_m(h as f64)) - h as f64).abs() < 1e-6);
    }
    assert!((pressure_altitude_dm(89_875) - 10_000).abs() <= 1);
}

#[test]
fn gnss_sigma_from_the_fix() {
    assert_eq!(gnss_sigma_m(0.6, 20), Some(5.0));
    assert!((gnss_sigma_m(2.0, 8).unwrap() - 13.6).abs() < 1e-9);
    assert_eq!(gnss_sigma_m(1.0, 5), None);
    assert_eq!(gnss_sigma_m(4.5, 12), None);
    assert_eq!(gnss_sigma_m(0.0, 12), None);
}

#[test]
fn no_altitude_without_a_reference() {
    let mut alt = Altimeter::default();
    assert_eq!(alt.gnss(300.0, 6.0), Gnss::NoBaro);
    alt.set_altitude(300.0, 1.0);
    assert_eq!(alt.altitude_m(), None);
    for _ in 0..300 {
        alt.baro(0.2, pressure_at_m(420.0));
    }
    assert_eq!(alt.altitude_m(), None);
    alt.set_altitude(300.0, 1.0);
    assert!((alt.altitude_m().unwrap() - 300.0).abs() < 1e-9);
    assert!((alt.bias_m() - 120.0).abs() < 0.1);
    // the barometric shape is kept: 10 m of pressure altitude up at 0.5 m/s is 10 m up
    for i in 0..400 {
        alt.baro(0.2, pressure_at_m(420.0 + (i as f64 * 0.1).min(10.0)));
    }
    assert!((alt.altitude_m().unwrap() - 310.0).abs() < 0.5);
}

#[test]
fn converges_from_the_pressure_altitude_bias() {
    let mut sim = Sim::new(1);
    let mut first = None;
    sim.run(
        600,
        |_| at(500.0, 120.0),
        |t, truth, alt, _| {
            first.get_or_insert(t);
            if t >= 60 {
                assert!(error(truth, alt) < 10.0, "{t} s: {:?}", alt.altitude_m());
            }
        },
    );
    assert_eq!(first, Some(0));
    assert!(error(500.0, &sim.alt) < 5.0, "{:?}", sim.alt.altitude_m());
    assert!((sim.alt.bias_m() - 120.0).abs() < 5.0);
}

#[test]
fn follows_the_weather_drift() {
    // 10 m/h of pressure altitude (a front, ~1.2 hPa/h) over 3 h, flat
    let mut sim = Sim::new(2);
    let mut e = Errors::default();
    sim.run(
        3 * 3600,
        |t| at(200.0, -50.0 + 10.0 * t as f64 / 3600.0),
        |t, truth, alt, _| {
            if t >= 600 {
                e.add(alt.altitude_m().unwrap() - truth);
            }
        },
    );
    e.check(2.0, 3.0, 9.0);
}

#[test]
fn rejects_a_gnss_jump() {
    let mut sim = Sim::new(3);
    sim.run(600, |_| at(800.0, 30.0), |_, _, _, _| {});
    let before = sim.alt.altitude_m().unwrap();
    let mut rejected = 0;
    let mut worst: f64 = 0.0;
    sim.run(
        120,
        |t| At {
            gnss_extra: Some(if (600..620).contains(&t) { 80.0 } else { 0.0 }),
            ..at(800.0, 30.0)
        },
        |_, _, alt, r| {
            rejected += (r == Some(Gnss::Rejected)) as u32;
            worst = worst.max((alt.altitude_m().unwrap() - before).abs());
        },
    );
    assert_eq!(rejected, 20);
    assert!(worst < 3.0, "moved {worst:.2} m");
}

#[test]
fn resets_on_a_lasting_disagreement() {
    // the barometer jumps by 100 m (a blocked vent, a sensor reset) and stays there: the GNSS disagrees for good
    let mut sim = Sim::new(4);
    sim.run(600, |_| at(800.0, 30.0), |_, _, _, _| {});
    let mut reset_at = None;
    sim.run(
        240,
        |_| at(800.0, 130.0),
        |t, _, _, r| {
            if r == Some(Gnss::Reset) {
                reset_at.get_or_insert(t);
            }
        },
    );
    let reset_at = reset_at.expect("no reset");
    assert!(
        (600 + 59..600 + 70).contains(&reset_at),
        "reset at {reset_at}"
    );
    assert!(error(800.0, &sim.alt) < 10.0, "{:?}", sim.alt.altitude_m());
}

#[test]
fn follows_a_climb() {
    // flat 10 min, 0.5 m/s for 20 min (600 m), flat again
    let climb = |t: u32| 300.0 + 0.5 * (t.clamp(600, 1800) - 600) as f64;
    let mut sim = Sim::new(5);
    let mut e = Errors::default();
    let mut vz = Errors::default();
    sim.run(
        2400,
        |t| at(climb(t), 60.0),
        |t, truth, alt, _| {
            if t >= 600 {
                e.add(alt.altitude_m().unwrap() - truth);
            }
            if (630..1800).contains(&t) {
                vz.add(alt.vertical_speed_m_s() - 0.5);
            }
        },
    );
    e.check(2.0, 3.0, 9.0);
    // vertical speed, m/s: noisier than the altitude (accel_sigma is set for MTB drops)
    vz.check(0.02, 0.08, 0.25);
}

/// MTB: flat 10 min, then 60 s at −3 m/s with a 2 m drop (in 0.4 s) every 10 s, 2 min flat, 2 min at +1.5 m/s.
fn mtb(t: f64) -> f64 {
    let d = (t - 600.0).clamp(0.0, 60.0);
    let drops = (d / 10.0).floor() * 2.0 + ((d % 10.0) / 0.4).min(1.0) * 2.0;
    1000.0 - 3.0 * d - drops.min(12.0) + (t - 780.0).clamp(0.0, 120.0) * 1.5
}

/// Barometer at 5 Hz along `profile` (pressure altitude 40 m above), GNSS at 1 Hz; returns the bias after each
/// barometer sample and the worst tracking error of the barometer (`h + b` against the pressure altitude) after
/// 10 min.
fn baro_run(profile: impl Fn(f64) -> f64) -> (Vec<f64>, f64) {
    let mut alt = Altimeter::default();
    let mut rng = Rng(7);
    let sigma = gnss_sigma_m(1.0, 12).unwrap();
    let decay = (-1.0 / GNSS_TAU_S).exp();
    let (mut slow, mut bias, mut worst) = (0.0, vec![], 0.0f64);
    for k in 0..1200 * 5 {
        let h = profile(k as f64 / 5.0);
        alt.baro(0.2, pressure_at_m(h + 40.0) + BARO_NOISE_PA * rng.gauss());
        if k >= 600 * 5 {
            worst = worst.max((alt.altitude_m().unwrap() + alt.bias_m() - (h + 40.0)).abs());
        }
        if k % 5 == 4 {
            slow = slow * decay + (1.0 - decay * decay).sqrt() * GNSS_SLOW_M * rng.gauss();
            alt.gnss(h + slow + GNSS_WHITE_M * rng.gauss(), sigma);
        }
        bias.push(alt.bias_m());
    }
    (bias, worst)
}

#[test]
fn mtb_drops_go_to_the_altitude_not_the_bias() {
    let (mtb_bias, tracking) = baro_run(mtb);
    // same noise, flat: the bias must move the same way, the terrain is not weather
    let (flat_bias, _) = baro_run(|_| 1000.0);
    let leak = mtb_bias
        .iter()
        .zip(&flat_bias)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max);
    assert!(leak < 0.05, "bias leak {leak:.3} m");
    // a 2 m drop in 0.4 s lags for a moment, then is caught up
    assert!(tracking < 2.0, "tracking {tracking:.2} m");
}

#[test]
fn barometer_gap_and_lost_gnss() {
    // no GNSS for 30 min (a tunnel, a lost fix): the barometer alone holds the altitude
    let mut sim = Sim::new(6);
    sim.run(600, |_| at(500.0, 10.0), |_, _, _, _| {});
    sim.run(
        1800,
        |t| At {
            gnss_extra: None,
            ..at(500.0 + (t - 600) as f64 * 0.1, 10.0)
        },
        |_, _, _, _| {},
    );
    assert!(error(680.0, &sim.alt) < 5.0, "{:?}", sim.alt.altitude_m());
    // a 30 s gap in the barometer stream: no jump at the next sample
    let h = sim.alt.altitude_m().unwrap();
    sim.alt.baro(30.0, pressure_at_m(690.0));
    assert!((sim.alt.altitude_m().unwrap() - h).abs() < 2.0);
}
