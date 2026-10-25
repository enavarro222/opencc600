//! Automatic timer events, manual precedence, and their independently decoded FIT totals.
use fit::Record;
use fitparser::{Value, profile::MesgNum};
use recorder::{Odometer, Recorder};
use ride_store::{
    littlefs2::fs::{Allocation, Filesystem},
    ram::RamFlash,
};

type Flash = RamFlash<32>;
const ID: u32 = 1_792_399_227;
const T0: u32 = 1000;

#[test]
fn threshold_missing_speed_settings_and_manual_pause() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    let mut r = Recorder::start(&fs, ID, T0, 42).unwrap();
    // 1.5 km/h is 416.666… mm/s: compare without rounding down the threshold.
    r.auto_pause(&fs, T0, Some(416), Some(15)).unwrap();
    assert!(r.auto_paused() && r.active() && !r.summary().running());
    r.auto_pause(&fs, T0 + 1, None, Some(15)).unwrap();
    assert!(r.auto_paused(), "no speed is not evidence of movement");
    r.auto_pause(&fs, T0 + 2, Some(417), Some(15)).unwrap();
    assert!(!r.auto_paused() && r.summary().running());
    r.auto_pause(&fs, T0 + 3, None, Some(15)).unwrap();
    assert!(r.summary().running(), "no speed is not evidence of a stop");
    r.auto_pause(&fs, T0 + 4, Some(900), Some(36)).unwrap();
    assert!(r.auto_paused(), "the new threshold applies immediately");
    r.auto_pause(&fs, T0 + 5, Some(1000), Some(36)).unwrap();
    assert!(r.summary().running(), "equality resumes");
    r.auto_pause(&fs, T0 + 6, Some(0), Some(15)).unwrap();
    assert!(r.auto_paused());
    r.auto_pause(&fs, T0 + 7, None, None).unwrap();
    assert!(
        r.summary().running(),
        "disabling auto-pause releases it even without a speed"
    );
    r.auto_pause(&fs, T0 + 8, Some(0), None).unwrap();
    assert!(r.summary().running());
    r.auto_pause(&fs, T0 + 9, Some(0), Some(15)).unwrap();
    r.pause(&fs, T0 + 10).unwrap();
    assert!(!r.active() && !r.auto_paused());
    r.auto_pause(&fs, T0 + 11, Some(8000), Some(15)).unwrap();
    r.auto_pause(&fs, T0 + 12, Some(8000), None).unwrap();
    assert!(
        !r.active(),
        "neither speed nor disabling auto-pause releases a manual pause"
    );
    r.resume(&fs, T0 + 13).unwrap();
    assert!(r.active() && r.summary().running());
}

#[test]
fn moving_and_elapsed_times_survive_reopen() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    let mut r = Recorder::start(&fs, ID, T0, 42).unwrap();
    let mut odo = Odometer::default();
    // 10 seconds moving, 10 stopped, then another 10 moving. Stop during the second automatic pause.
    for i in 0..=30 {
        let speed = if (10..20).contains(&i) || i == 30 {
            0
        } else {
            8000
        };
        r.auto_pause(&fs, T0 + i, Some(speed), Some(15)).unwrap();
        let distance_cm = odo.update(T0 + i, Some(speed), r.summary().running());
        r.record(
            &fs,
            &Record {
                timestamp: T0 + i,
                speed_mm_s: Some(speed),
                distance_cm: Some(distance_cm),
                ..Default::default()
            },
        )
        .unwrap();
        if i == 15 {
            let t = r.summary().totals_at(T0 + i);
            assert_eq!((t.timer_ms, t.elapsed_ms), (10_000, 15_000));
        }
    }
    assert!(r.auto_paused());
    let before = r.summary().totals();
    // A restarted device offers the ride manually paused, never resumes it just because it detects speed.
    let mut r = Recorder::reopen(&fs, ID).unwrap();
    assert_eq!(r.summary().totals(), before);
    assert!(!r.active());
    r.auto_pause(&fs, T0 + 35, Some(8000), Some(15)).unwrap();
    assert!(!r.active());
    let totals = r.stop(&fs, T0 + 40, 0).unwrap();
    assert_eq!((totals.timer_ms, totals.elapsed_ms), (20_000, 40_000));
    let mut size = 0;
    ride_store::finished(&fs, |_, n| size = n).unwrap();
    let mut file = vec![0; size];
    ride_store::read(&fs, ID, true, &mut file).unwrap();
    let messages = fitparser::from_bytes(&file).unwrap();
    let session = messages
        .iter()
        .find(|m| m.kind() == MesgNum::Session)
        .unwrap();
    let number = |name: &str| match session
        .fields()
        .iter()
        .find(|f| f.name() == name)
        .unwrap()
        .value()
    {
        Value::Float64(v) => *v,
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(number("total_timer_time"), 20.0);
    assert_eq!(number("total_elapsed_time"), 40.0);
    assert!((number("enhanced_avg_speed") - number("total_distance") / 20.0).abs() < 0.001);
}

#[test]
fn slow_movement_above_the_configured_threshold_counts_distance() {
    let mut odo = Odometer::default();
    odo.update_with_min_speed(T0, Some(450), true, 417);
    assert_eq!(odo.update_with_min_speed(T0 + 1, Some(450), true, 417), 45);
    assert_eq!(odo.update_with_min_speed(T0 + 2, Some(400), false, 417), 45);
    assert_eq!(odo.update_with_min_speed(T0 + 20, Some(450), true, 417), 45);
}

#[test]
fn saving_during_auto_pause_keeps_total_time_and_moving_average() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    let mut r = Recorder::start(&fs, ID, T0, 42).unwrap();
    r.record(
        &fs,
        &Record {
            timestamp: T0 + 9,
            distance_cm: Some(10_000),
            ..Default::default()
        },
    )
    .unwrap();
    r.auto_pause(&fs, T0 + 10, Some(0), Some(15)).unwrap();
    assert!(r.auto_paused());
    let totals = r.stop(&fs, T0 + 40, 0).unwrap();
    assert_eq!((totals.timer_ms, totals.elapsed_ms), (10_000, 40_000));
    assert_eq!(totals.avg_speed_mm_s(), 10_000);
}
