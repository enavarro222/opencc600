//! Every file is decoded again by `fitparser`, an independent implementation of the FIT SDK profile: it checks
//! the header, the CRCs, the field numbers (names) and the scales.

use fit::*;
use fitparser::profile::MesgNum;
use fitparser::{FitDataRecord, Value};

const START_UNIX: u32 = 1_792_399_227; // 2026-10-19 08:40:27 UTC

fn put<M: Message>(file: &mut Vec<u8>, m: &M) {
    let mut buf = [0u8; 128];
    let n = m.encode(&mut buf).unwrap();
    file.extend_from_slice(&buf[..n]);
}

/// A complete 60 s ride, as the recorder writes it.
fn ride() -> Vec<u8> {
    let t0 = timestamp(START_UNIX);
    let mut file = header(0).to_vec();
    let mut buf = [0u8; 512];
    let n = encode_definitions(&mut buf).unwrap();
    file.extend_from_slice(&buf[..n]);
    put(
        &mut file,
        &FileId {
            serial_number: 1234,
            time_created: t0,
        },
    );
    put(
        &mut file,
        &Event {
            timestamp: t0,
            kind: EventType::Start,
        },
    );
    for i in 0..60 {
        put(
            &mut file,
            &Record {
                timestamp: t0 + i,
                lat_e7: Some(450_000_000 + i as i32 * 100),
                lon_e7: Some(-60_000_000),
                altitude_dm: Some(2_345),
                speed_mm_s: Some(8_333),
                distance_cm: Some(i * 833),
                temperature_c: Some(-3),
                heart_rate: (i > 10).then_some(120),
                ..Default::default()
            },
        );
    }
    let totals = Totals {
        start_time: t0,
        end_time: t0 + 59,
        elapsed_ms: 59_000,
        timer_ms: 59_000,
        distance_cm: 59 * 833,
        ascent_m: 12,
        descent_m: 3,
        max_speed_mm_s: 8_333,
    };
    put(
        &mut file,
        &Event {
            timestamp: t0 + 59,
            kind: EventType::StopAll,
        },
    );
    put(&mut file, &Lap(totals));
    put(
        &mut file,
        &Session {
            totals,
            num_laps: 1,
        },
    );
    put(
        &mut file,
        &Activity {
            timestamp: t0 + 59,
            local_timestamp: t0 + 59 + 7200,
            timer_ms: 59_000,
        },
    );
    let data = (file.len() - HEADER_LEN) as u32;
    file[..HEADER_LEN].copy_from_slice(&header(data));
    let crc = Crc::new().update(&file).value();
    file.extend_from_slice(&crc.to_le_bytes());
    file
}

fn field<'a>(r: &'a FitDataRecord, name: &str) -> &'a Value {
    let f = r.fields().iter().find(|f| f.name() == name);
    f.unwrap_or_else(|| panic!("{:?}: no {name} in {:?}", r.kind(), r.fields()))
        .value()
}

fn f64_of(v: &Value) -> f64 {
    match v {
        Value::Float64(x) => *x,
        Value::UInt32(x) => *x as f64,
        Value::UInt16(x) => *x as f64,
        Value::UInt8(x) => *x as f64,
        Value::SInt8(x) => *x as f64,
        Value::SInt32(x) => *x as f64,
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn crc_is_arc() {
    assert_eq!(Crc::new().update(b"123456789").value(), 0xBB3D);
}

#[test]
fn whole_ride_decodes() {
    let file = ride();
    // For a second check with the official Garmin decoder: `FIT_OUT=x.fit cargo test`, `tools/check_fit.py x.fit`.
    if let Some(path) = std::env::var_os("FIT_OUT") {
        std::fs::write(path, &file).unwrap();
    }
    let records = fitparser::from_bytes(&file).expect("independent decoder");
    let kinds: Vec<MesgNum> = records.iter().map(|r| r.kind()).collect();
    assert_eq!(kinds[0], MesgNum::FileId);
    assert_eq!(kinds.iter().filter(|k| **k == MesgNum::Record).count(), 60);
    assert_eq!(
        &kinds[kinds.len() - 4..],
        &[
            MesgNum::Event,
            MesgNum::Lap,
            MesgNum::Session,
            MesgNum::Activity
        ]
    );

    let id = &records[0];
    assert_eq!(field(id, "type"), &Value::String("activity".into()));
    assert_eq!(
        field(id, "manufacturer"),
        &Value::String("development".into())
    );
    assert_eq!(field(id, "serial_number"), &Value::UInt32z(1234));

    let rec: Vec<_> = records
        .iter()
        .filter(|r| r.kind() == MesgNum::Record)
        .collect();
    let r = rec[0];
    match field(r, "timestamp") {
        Value::Timestamp(t) => assert_eq!(t.timestamp(), START_UNIX as i64),
        v => panic!("{v:?}"),
    }
    // semicircles, degrees × 2³¹ / 180
    assert_eq!(field(r, "position_lat"), &Value::SInt32(536_870_912));
    assert!((f64_of(field(r, "position_long")) - (-6.0 * 2f64.powi(31) / 180.0)).abs() <= 1.0);
    assert!(
        (f64_of(field(r, "enhanced_altitude")) - 234.5).abs() < 0.2,
        "{:?}",
        field(r, "enhanced_altitude")
    );
    assert!((f64_of(field(r, "enhanced_speed")) - 8.333).abs() < 1e-6);
    assert_eq!(f64_of(field(r, "temperature")), -3.0);
    // invalid values are dropped by the decoder: no heart rate before 10 s, no power at all
    assert!(
        r.fields()
            .iter()
            .all(|f| f.name() != "heart_rate" && f.name() != "power")
    );
    assert_eq!(f64_of(field(rec[30], "heart_rate")), 120.0);
    assert!((f64_of(field(rec[59], "distance")) - 491.47).abs() < 1e-6);

    let session = records
        .iter()
        .find(|r| r.kind() == MesgNum::Session)
        .unwrap();
    assert_eq!(field(session, "sport"), &Value::String("cycling".into()));
    assert!((f64_of(field(session, "total_timer_time")) - 59.0).abs() < 1e-6);
    assert!((f64_of(field(session, "total_distance")) - 491.47).abs() < 1e-6);
    assert_eq!(f64_of(field(session, "total_ascent")), 12.0);
    assert!((f64_of(field(session, "enhanced_avg_speed")) - 8.33).abs() < 0.01);
    assert_eq!(f64_of(field(session, "num_laps")), 1.0);
    let lap = records.iter().find(|r| r.kind() == MesgNum::Lap).unwrap();
    assert_eq!(f64_of(field(lap, "total_descent")), 3.0);
    let activity = records.last().unwrap();
    assert_eq!(f64_of(field(activity, "num_sessions")), 1.0);
}

#[test]
fn corrupted_file_is_rejected() {
    let mut file = ride();
    file[100] ^= 1;
    assert!(fitparser::from_bytes(&file).is_err());
}

#[test]
fn semicircle_rounding() {
    assert_eq!(semicircles(0), 0);
    assert_eq!(semicircles(1_800_000_000), i32::MAX.wrapping_add(1)); // 180° wraps, as in FIT
    assert_eq!(semicircles(900_000_000), 1 << 30);
    assert_eq!(semicircles(-900_000_000), -(1 << 30));
    // 1e-7° ≈ 1.19 semicircles
    assert_eq!(semicircles(1), 1);
    assert_eq!(semicircles(-1), -1);
}

#[test]
fn small_buffer_is_an_error() {
    let mut buf = [0u8; 8];
    assert_eq!(Record::default().encode(&mut buf), Err(Full));
    assert_eq!(encode_definitions(&mut buf), Err(Full));
}

#[test]
fn messages_read_back() {
    let r = Record {
        timestamp: 77,
        lat_e7: Some(451_234_567),
        lon_e7: Some(-61_234_567),
        altitude_dm: Some(-123),
        speed_mm_s: Some(5_000),
        distance_cm: Some(42),
        temperature_c: Some(-5),
        heart_rate: None,
        cadence: Some(90),
        power_w: Some(250),
    };
    let mut buf = [0u8; 64];
    let n = r.encode(&mut buf).unwrap();
    assert_eq!(record_len(buf[0], None), Some(n));
    let back = Record::decode(&buf[..n]).unwrap();
    // altitude in 0.2 m steps, positions within 1e-7°
    assert_eq!(back.altitude_dm, Some(-124));
    assert!((back.lat_e7.unwrap() - r.lat_e7.unwrap()).abs() <= 1);
    assert!((back.lon_e7.unwrap() - r.lon_e7.unwrap()).abs() <= 1);
    assert_eq!(
        Record {
            lat_e7: r.lat_e7,
            lon_e7: r.lon_e7,
            altitude_dm: r.altitude_dm,
            ..back
        },
        r
    );
    assert_eq!(Event::decode(&buf[..n]), None);

    let e = Event {
        timestamp: 9,
        kind: EventType::Stop,
    };
    let n = e.encode(&mut buf).unwrap();
    assert_eq!(Event::decode(&buf[..n]), Some(e));
    assert!(!Activity::is(&buf[..n]));

    let mut defs = [0u8; 512];
    let n = encode_definitions(&mut defs).unwrap();
    let mut pos = 0;
    while pos < n {
        pos += record_len(defs[pos], defs.get(pos + 5).copied()).unwrap();
    }
    assert_eq!(pos, n);
}
