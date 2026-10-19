# Altitude and elevation gain (D+)

How opencc600 combines the barometer and the GNSS altitude. Code: `crates/altitude` (filter), `crates/recorder`
(D+), `firmware/src/bin/main.rs` (wiring). Requirements: [PRODUCT.md](PRODUCT.md#altitude-and-elevation-gain-d).

## 1. The two sensors

| | Barometer (via the nRF, 5 Hz) | GNSS altitude (GGA, 1 Hz) |
|---|---|---|
| Short-term noise | 0.8 Pa ≈ **7 cm** at rest ([REVERSE_NOTES.md](REVERSE_NOTES.md)) | **5–15 m**, jumps under trees, in valleys (multipath) |
| Long-term | **drifts with the weather**: 1 hPa ≈ 8.3 m; a few m/h, more than 10 m/h with a front | no drift on average, but errors correlated over minutes |
| Absolute level | standard atmosphere vs real sea-level pressure: off by up to ±200 m | right on average (GGA gives the height above the geoid, MSL) |
| Scale | Δh off by ~3.5 % per 10 °C of difference with the standard atmosphere | right |

They are complementary: the barometer has the **shape** of the profile, the GNSS has the **level**. The serious
implementations (PX4 EKF2, ArduPilot EKF3, and, from their behaviour, the bike computers with a barometer) keep the barometric shape and estimates
its offset (the *bias*) from the GNSS, slowly.

## 2. Pressure to altitude

International standard atmosphere, troposphere ([US Standard Atmosphere 1976][usstd]):

    h_baro = T0/L · (1 − (p / p0)^(R·L / g·M)) = 44330.77 m · (1 − (p / 101325 Pa)^0.190263)

`h_baro` is the *pressure altitude*. It differs from the true altitude by the bias `b`, which the filter
estimates. Locally, a bias in metres is as good as a wrong `p0`.

## 3. The filter: a 3-state Kalman filter

State `x = [h, vz, b]`: altitude (m, MSL), vertical speed (m/s), barometer bias (m). This is the classic
loosely-coupled baro / GNSS height filter ([Groves][groves], baro-aided navigation; [PX4 EKF2][px4] estimates the same
baro bias next to its height state).

**Prediction** at each barometer sample (Δt ≈ 0.2 s; Δt capped at 5 s after a gap):

    h  ← h + vz·Δt
    vz ← vz                      vertical acceleration: white noise, σa = 1 m/s² (piecewise constant over Δt)
    b  ← b                       random walk, 4 m/√h (weather)

    F = [[1, Δt, 0], [0, 1, 0], [0, 0, 1]]
    Q = σa² · [[Δt⁴/4, Δt³/2, 0], [Δt³/2, Δt², 0], [0, 0, 0]] + diag(0, 0, q_b·Δt)

The discrete white noise acceleration model is the one of [Bar-Shalom et al.][barshalom].

**Barometer update**: `z = h_baro = h + b`, `H = [1, 0, 1]`, σ = 0.3 m. The sensor gives 7 cm. The extra
covers wind on the vent, hands on the case and the dynamic pressure (½ρv² ≈ 75 Pa ≈ 6 m at 40 km/h into a headwind,
part of which reaches the sensor).

**GNSS update**: `z = h_gnss`, `H = [1, 0, 0]`, with a variance from the fix itself:

- vertical σ = 1.7 × HDOP × 4 m (VDOP is typically 1.5–2 × HDOP, user range error ~4 m), at least 5 m. The
  receiver sends no VDOP or vertical accuracy in its trimmed NMEA output, so HDOP stands in;
- no update with fewer than 6 satellites used or HDOP above 4;
- **correlated errors**: GNSS altitude errors last for minutes (satellite geometry, multipath), but a Kalman
  filter takes the 1 Hz samples as independent and would trust their average too much. The usual cure is to
  inflate the measurement variance ([Bar-Shalom et al.][barshalom], [Groves][groves]): σ × 3 here. With the
  bias random walk this sets the time constant of the bias at √(R/q_b) = 20.4 m / (4 m/√h) ≈ 5 minutes (HDOP 1).
  After one minute of fix the formal σ is ~3 m. The real error is larger, because the GNSS errors are correlated
  (tests: < 10 m).

The covariance update uses `P ← P − K·S·Kᵀ` in f64. The ESP32-S3 has no double-precision FPU, but 5 Hz × a 3×3
matrix costs nothing in software, and f64 removes the cancellation of the start (bias σ 300 m against a 0.3 m sensor).

**Outliers (gating)**: a GNSS altitude whose innovation `|z − h|` is above 3 × √(P_hh + σ²) (σ not inflated) is
rejected. If 60 epochs in a row are rejected, the filter trusts the GNSS again. This is the innovation-consistency
check of PX4, which also resets its height on the reference after a timeout. The reset puts `h` on the GNSS altitude and keeps
the barometric shape (`h + b` is unchanged).

**Start**: at the first barometer sample, `h = h_baro`, `b = 0`, with σ 300 m on both and the correlation that
says `h + b` is known. Until a reference arrives (an accepted GNSS altitude, or `set_altitude` with a known
altitude, see *Later*), the filter reports no altitude: a pressure altitude off by 100 m is worse than nothing.
Without a barometer (its stream is not on), the firmware records the raw GNSS altitude.

### MTB: sudden altitude changes

Steep descents (−3 m/s) and drops (2 m in 0.4 s) are much faster than on the road. Two risks:

- **The bias absorbing the terrain.** It does not: the barometer measures `h + b`, and only the GNSS can move `b`
  apart from `h`. On a simulated MTB descent the bias differs by less than 1 cm from the same ride on the flat
  with the same noise.
- **Lag.** The filter follows the barometer through the vertical speed, limited by `accel_sigma`. A 2 m drop lags
  by up to 2.0 m for a moment at 0.3 m/s², 1.4 m at 1 m/s², 0.8 m at 3 m/s², then is caught up. 1 m/s² is the
  compromise. The vertical speed is not meant for the gradient display, which should be Δh/Δd over 50–100 m.

The D+ does not go through the filter (section 4): a sudden drop counts at once, at the 1 Hz of the records.

### Tuning summary

| Parameter | Value | Effect when larger |
|---|---|---|
| `accel_sigma` | 1 m/s² | follows MTB drops closer; vz noisier (at rest: 0.02 m/s RMS at 0.3, 0.05 at 1, 0.11 at 3) |
| `baro_sigma_m` | 0.3 m | smoother altitude, slower to follow the barometer |
| `bias_walk` | 4 m/√h | the bias follows the weather faster, more GNSS noise on the altitude |
| `gnss_inflation` | 3 | slower and smoother recalibration |
| `gate` | 3 σ | fewer GNSS rejections |
| `reset_after` | 60 epochs | longer before trusting a GNSS that disagrees |
| `init_bias_sigma_m` | 300 m | (start only) |

## 4. Elevation gain (D+ / D−): from the pressure only

The D+ is computed on the **pressure altitude, not on the filtered altitude**. Every change of the bias estimate
(initial convergence, GNSS noise reaching the bias) would otherwise count as climbing. That is ~10 m at each start,
plus a few metres per hour.

- Each FIT record carries the pressure (`absolute_pressure`, field 91 of `record`, in Pa, [FIT SDK profile][fit]).
  The sensor noise (0.8 Pa) is below the field's 1 Pa resolution.
- `recorder::Summary` turns it into a pressure altitude (dm) and counts a climb or a descent once it passes the
  **2 m hysteresis**. The totals come from the file's own records, so the recovery after a power cut, which replays
  the file, gets the same D+.
- The FIT `enhanced_altitude` is the filtered altitude (the absolute profile). Without a reference yet, it is the
  raw GNSS altitude.

Known limits of this choice:

- **Weather drift** during the ride counts: a front lowering the pressure by 1 hPa over a flat ride gives ~8 m of
  D+. Subtracting the estimated bias would remove it but add the GNSS noise. Any D+ from a barometer has this limit.
- **Temperature scale**: the standard atmosphere assumes 15 °C at sea level, −6.5 °C/km. On a hot day the real
  Δh is larger: at 30 °C a 1000 m climb reads ~950 m. The case temperature (sun, pocket, car) does not tell the
  air temperature, so there is no correction yet.
- **Dynamic pressure**: a constant speed gives a constant offset, which does not count. A sudden sprint into the
  wind can look like a ~1–2 m step. That is under the hysteresis.

## 5. Tests

`crates/altitude/tests/filter.rs`, with synthetic series (seeded noise):

- converges from a 120 m pressure-altitude bias: < 10 m after one minute, < 5 m after 10 minutes;
- follows a 10 m/h weather drift over 3 h: mean error < 2 m, RMS < 3 m, worst < 9 m;
- follows a 0.5 m/s climb with the same bounds; vertical speed error < 0.08 m/s RMS, < 0.25 m/s at worst;
- rejects a 20 s GNSS jump of 80 m (moves < 3 m), and resets after a minute of lasting disagreement;
- MTB descent with drops: the bias does not move (< 5 cm against the flat ride), the barometer is tracked
  within 2 m;
- holds the altitude 30 min without GNSS, and does not jump after a 30 s gap in the barometer stream;
- no altitude without a reference; `set_altitude` gives one and keeps the barometric shape.

The GNSS error of the tests is 4 m white plus 5 m correlated over 60 s. Over 20 seeds, flat or climbing, the
altitude error after convergence is ~2 m RMS, ~8 m at worst. What is left is the GNSS level error: the filter
cannot know the level better than a few minutes of GNSS average.

`crates/recorder/tests`: the D+ of a recorded ride comes from the pressure, and a flat ride with sensor noise
gives no D+.

## 6. Later

- **Post-ride smoothing** (Rauch–Tung–Striebel, [Rauch et al. 1965][rts]): running the filter backward over the
  ride gives the bias the benefit of the GNSS that follows. It improves the absolute altitude of the first minutes.
  It could rewrite `enhanced_altitude` when the file is sealed.
- **Known altitude at start**: same place as the last ride's end (position within ~50 m), or a saved place:
  `Altimeter::set_altitude` with a small σ. The altitude is then right from the first second.
- **Route / DEM profile** (Copernicus GLO-30) when a route is loaded: one more measurement of `h`, with σ ~ 5 m.
- **Vertical speed from the GNSS Doppler** (UBX-style `velD`): not in this receiver's NMEA output.
- **Temperature scale factor** as a 4th state: observable only over large climbs with a good GNSS.

## References

- [usstd]: U.S. Standard Atmosphere, 1976. NOAA / NASA / USAF, NOAA-S/T 76-1562.
- [groves]: P. D. Groves, *Principles of GNSS, Inertial, and Multisensor Integrated Navigation Systems*, 2nd ed.,
  Artech House, 2013 (barometric altimeter, integration, correlated measurement noise).
- [barshalom]: Y. Bar-Shalom, X. R. Li, T. Kirubarajan, *Estimation with Applications to Tracking and Navigation*,
  Wiley, 2001 (white noise acceleration models, gating).
- [rts]: H. E. Rauch, F. Tung, C. T. Striebel, "Maximum likelihood estimates of linear dynamic systems", *AIAA
  Journal* 3(8), 1965.
- [px4]: PX4 EKF2 tuning guide, height sources and baro bias:
  <https://docs.px4.io/main/en/advanced_config/tuning_the_ecl_ekf.html>
- [fit]: Garmin FIT SDK, `Profile.xlsx` (message `record`, field `absolute_pressure`): <https://developer.garmin.com/fit/>

[usstd]: #references
[groves]: #references
[barshalom]: #references
[rts]: #references
[px4]: https://docs.px4.io/main/en/advanced_config/tuning_the_ecl_ekf.html
[fit]: https://developer.garmin.com/fit/
