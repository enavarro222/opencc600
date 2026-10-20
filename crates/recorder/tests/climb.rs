//! Ascent and descent totals on altitude profiles that have fooled, or could fool, the count: what the recorder
//! counts from the FIT `absolute_pressure` of each record (1 Hz), with the sensor noise (±1 Pa).

use fit::{Record, timestamp};
use recorder::Summary;

/// FIT `absolute_pressure` at a pressure altitude in decimetres.
fn pressure(alt_dm: f64) -> u32 {
    altitude::pressure_at_m(alt_dm / 10.0).round() as u32
}

/// Totals (m) of a profile, one altitude (dm) a second.
fn totals(profile: &[f64]) -> (u16, u16) {
    let t0 = timestamp(1_792_478_595);
    let mut s = Summary::default();
    s.event(&fit::Event {
        timestamp: t0,
        kind: fit::EventType::Start,
    });
    let mut x = 7u32;
    for (i, &alt) in profile.iter().enumerate() {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise_pa = (x >> 30) as i32 - 1;
        s.record(&Record {
            timestamp: t0 + i as u32,
            pressure_pa: Some((pressure(alt) as i32 + noise_pa) as u32),
            ..Default::default()
        });
    }
    let t = s.totals();
    (t.ascent_m, t.descent_m)
}

/// Appends `seconds` seconds going from the last altitude to `to_dm`, evenly.
fn go(p: &mut Vec<f64>, to_dm: f64, seconds: u32) {
    let from = *p.last().unwrap_or(&to_dm);
    for k in 1..=seconds {
        p.push(from + (to_dm - from) * k as f64 / seconds as f64);
    }
}

fn near(got: u16, want: u16, tolerance: u16) -> bool {
    got.abs_diff(want) <= tolerance
}

/// A train ride (2026-10-20, `dump/train/`), the CC600 in a pocket: two tunnels each
/// raised the pressure altitude by 45 to 50 m within 5 to 10 s, for 15 to 30 s, then back (the pressure wave of a
/// train entering a tunnel; the GNSS lost the fix at the same time). The ride counted 251 m up / 207 m down where
/// the line climbs ~130 m (EU-DEM under the positions, smoothed: 159 / 128 m, the hills over the tunnels
/// included; the rule here gives 129 / 85 m on the recorded pressures). Here: the line climbs 60 m in 4 minutes,
/// then a tunnel; later, flat, another one.
#[test]
fn a_tunnel_in_a_train_is_not_a_climb() {
    let mut p = vec![1_200.0];
    go(&mut p, 1_800.0, 240);
    // the first tunnel: up 46 m in 6 s, 15 s up there with a jitter of a metre, back in 8 s
    let base = *p.last().unwrap();
    go(&mut p, base + 460.0, 6);
    for k in 0..15 {
        p.push(base + 460.0 + if k % 2 == 0 { 10.0 } else { -10.0 });
    }
    go(&mut p, base, 8);
    go(&mut p, base, 300);
    // the second one, longer: up 50 m, 30 s, back
    go(&mut p, base + 500.0, 8);
    go(&mut p, base + 500.0, 30);
    go(&mut p, base, 10);
    go(&mut p, base, 120);
    let (up, down) = totals(&p);
    assert!(near(up, 60, 2) && down <= 2, "{up} up, {down} down");
}

/// What the tunnel rule must never touch: a fast mountain bike descent. 600 m down in 2 min 30 (4 m/s, 6 m/s in
/// the steepest part), three bumps of 4 m climbed at 0.5 m/s on the way, and a drop into a gully then out of it.
#[test]
fn a_fast_descent_counts_whole() {
    let mut p = vec![15_000.0];
    go(&mut p, 13_000.0, 50); // 4 m/s
    go(&mut p, 13_040.0, 8); // a bump, 0.5 m/s
    go(&mut p, 11_000.0, 34); // 6 m/s
    go(&mut p, 11_040.0, 8);
    go(&mut p, 10_000.0, 26);
    go(&mut p, 10_040.0, 8);
    go(&mut p, 9_000.0, 25);
    // the gully: down 15 m at 5 m/s, up 15 m at 1 m/s (standing on the pedals)
    go(&mut p, 8_850.0, 3);
    go(&mut p, 9_000.0, 15);
    go(&mut p, 9_000.0, 30);
    let (up, down) = totals(&p);
    assert!(near(down, 600 + 12 + 15, 3) && near(up, 12 + 15, 3), "{up} up, {down} down");
}

/// Short steep climbs, one after the other: 20 bumps of 10 m climbed at 1 m/s (a sprint up a short wall), each
/// followed by a descent at 2.5 m/s. The 2 m hysteresis loses a part of each turn (187 m counted, with or without
/// the tunnel rule): the rule must not lose more.
#[test]
fn short_steep_climbs_count() {
    let mut p = vec![3_000.0];
    for _ in 0..20 {
        go(&mut p, 3_100.0, 10);
        go(&mut p, 3_000.0, 4);
    }
    go(&mut p, 3_000.0, 10);
    let (up, down) = totals(&p);
    assert!(near(up, 187, 2) && near(down, 187, 2), "{up} up, {down} down");
}

/// A rise faster than any rider that does not fall back is real (a lift, a car with the ride running): held for a
/// minute, then counted.
#[test]
fn a_lift_counts_after_a_minute() {
    let mut p = vec![10_000.0];
    go(&mut p, 13_000.0, 60); // 300 m at 5 m/s
    go(&mut p, 13_000.0, 90);
    let (up, down) = totals(&p);
    assert!(near(up, 300, 2) && down <= 1, "{up} up, {down} down");
}
