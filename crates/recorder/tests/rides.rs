//! Rides recorded on the RAM flash, read back with `fitparser` (independent decoder); the power is cut at every
//! flash operation of a ride, and of the recovery that follows.

use fit::{Record, timestamp};
use fitparser::profile::MesgNum;
use fitparser::{FitDataRecord, Value};
use recorder::{
    EpochClock, FLUSH_EVERY_S, Odometer, Recorder, Recovered, Summary, interrupted, last_finished,
    recover,
};
use ride_store::littlefs2::fs::{Allocation, Filesystem};
use ride_store::littlefs2::io::Result;
use ride_store::ram::RamFlash;

type Flash = RamFlash<32>;

const ID: u32 = 1_792_399_227;
const SAMPLES: u32 = 120;
const PAUSE: core::ops::Range<u32> = 50..65;

fn t0() -> u32 {
    timestamp(ID)
}

/// Sample `i`: 8 m/s, climbing 0.5 m/s for 60 s then down.
fn sample(i: u32) -> Record {
    let alt = if i < 60 {
        1000 + i as i32 * 5
    } else {
        1300 - (i as i32 - 60) * 5
    };
    Record {
        timestamp: t0() + i,
        lat_e7: Some(450_000_000 + i as i32 * 700),
        lon_e7: Some(60_000_000),
        altitude_dm: Some(alt),
        speed_mm_s: Some(8_000),
        distance_cm: Some(i * 800),
        temperature_c: Some(12),
        pressure_pa: Some(pressure(alt)),
        ..Default::default()
    }
}

/// FIT `absolute_pressure` at a pressure altitude in decimetres.
fn pressure(alt_dm: i32) -> u32 {
    altitude::pressure_at_m(alt_dm as f64 / 10.0).round() as u32
}

/// What the ride was told had been done.
#[derive(Debug, Default)]
struct Acked {
    started: bool,
    /// `record` calls that returned Ok (the last ones may still be in the RAM buffer).
    recorded: u32,
    stopped: bool,
}

fn ride(fs: &Filesystem<'_, Flash>, acked: &mut Acked) -> Result<()> {
    let mut r = Recorder::start(fs, ID, t0(), 42)?;
    acked.started = true;
    for i in 0..SAMPLES {
        if i == PAUSE.start {
            r.pause(fs, t0() + i)?;
        }
        if i == PAUSE.end {
            r.resume(fs, t0() + i)?;
        }
        r.record(fs, &sample(i))?;
        acked.recorded += 1;
    }
    r.stop(fs, t0() + SAMPLES - 1, 7200)?;
    acked.stopped = true;
    Ok(())
}

fn read_fit(fs: &Filesystem<'_, Flash>) -> Vec<u8> {
    let mut size = None;
    ride_store::finished(fs, |id, len| {
        assert_eq!(id, ID);
        size = Some(len);
    })
    .unwrap();
    let mut buf = vec![0; size.expect("no finished ride")];
    ride_store::read(fs, ID, true, &mut buf).unwrap();
    buf
}

fn num(r: &FitDataRecord, name: &str) -> f64 {
    let v = r
        .fields()
        .iter()
        .find(|f| f.name() == name)
        .map(|f| f.value());
    match v {
        Some(Value::Float64(x)) => *x,
        Some(Value::UInt32(x)) => *x as f64,
        Some(Value::UInt16(x)) => *x as f64,
        other => panic!("{name}: {other:?}"),
    }
}

/// Decodes the finished ride and checks it is a whole activity consistent with its records; returns the
/// timestamps of its records (FIT seconds from t0).
fn check_fit(file: &[u8], ctx: &str) -> Vec<u32> {
    let msgs = fitparser::from_bytes(file).unwrap_or_else(|e| panic!("{ctx}: undecodable: {e}"));
    let kinds: Vec<MesgNum> = msgs.iter().map(|m| m.kind()).collect();
    assert_eq!(kinds[0], MesgNum::FileId, "{ctx}");
    assert_eq!(*kinds.last().unwrap(), MesgNum::Activity, "{ctx}");
    let records: Vec<&FitDataRecord> = msgs
        .iter()
        .filter(|m| m.kind() == MesgNum::Record)
        .collect();
    let session = msgs
        .iter()
        .find(|m| m.kind() == MesgNum::Session)
        .expect("session");
    let times: Vec<u32> = records
        .iter()
        .map(|r| {
            match &r
                .fields()
                .iter()
                .find(|f| f.name() == "timestamp")
                .unwrap()
                .value()
            {
                Value::Timestamp(t) => (t.timestamp() as u32) - ID,
                v => panic!("{v:?}"),
            }
        })
        .collect();
    if let Some(&last) = times.last() {
        assert!(
            (num(session, "total_distance") - last as f64 * 8.0).abs() < 1e-6,
            "{ctx}"
        );
    }
    times
}

#[test]
fn whole_ride() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    ride(&fs, &mut Acked::default()).unwrap();
    let file = read_fit(&fs);
    if let Some(path) = std::env::var_os("FIT_OUT") {
        std::fs::write(path, &file).unwrap();
    }
    let times = check_fit(&file, "whole ride");
    // paused samples are not recorded
    let want: Vec<u32> = (0..SAMPLES).filter(|i| !PAUSE.contains(i)).collect();
    assert_eq!(times, want);

    let msgs = fitparser::from_bytes(&file).unwrap();
    let session = msgs.iter().find(|m| m.kind() == MesgNum::Session).unwrap();
    assert_eq!(num(session, "total_elapsed_time"), (SAMPLES - 1) as f64);
    assert_eq!(
        num(session, "total_timer_time"),
        (SAMPLES - 1 - PAUSE.len() as u32) as f64
    );
    // Recorded altitudes: 100 m → 124.5 m (pause at 50 s), 127.5 m at the resume (65 s), down to 100.5 m.
    // Hysteresis 2 m: ascent 100 → 124 → 127.5 = 27.5 m; descent in 2 m steps 127.5 → 101.5 = 26 m, the last
    // metre stays under the threshold (whole metres, floored).
    assert_eq!(num(session, "total_ascent"), 27.0);
    assert_eq!(num(session, "total_descent"), 26.0);
    let events: Vec<String> = msgs
        .iter()
        .filter(|m| m.kind() == MesgNum::Event)
        .map(|m| {
            format!(
                "{}",
                m.fields()
                    .iter()
                    .find(|f| f.name() == "event_type")
                    .unwrap()
                    .value()
            )
        })
        .collect();
    assert_eq!(events, ["start", "stop", "start", "stop_all"]);
}

fn mount_and_recover(flash: &mut Flash) -> Result<Vec<Recovered>> {
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, flash)?;
    let mut done = vec![];
    recover(&fs, 7200, None, |r| done.push(r))?;
    Ok(done)
}

/// After a cut at `cut` and a recovery: the ride is complete, or gone only if its start was never acknowledged.
fn check_after(flash: &mut Flash, acked: &Acked, ctx: &str) {
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, flash).unwrap_or_else(|e| panic!("{ctx}: mount {e:?}"));
    let mut unfinished = 0;
    ride_store::unfinished(&fs, |_, _| unfinished += 1).unwrap();
    assert_eq!(unfinished, 0, "{ctx}: still unfinished after recovery");
    let mut finished = 0;
    ride_store::finished(&fs, |_, _| finished += 1).unwrap();
    if finished == 0 {
        // nothing recorded yet: the empty ride is dropped
        assert!(
            acked.recorded <= FLUSH_EVERY_S + 1,
            "{ctx}: ride lost, {acked:?}"
        );
        return;
    }
    let times = check_fit(&read_fit(&fs), ctx);
    // everything acknowledged is there, except what was still in the RAM buffer (≤ one flush period)
    let kept = times.len() as u32;
    let acked_recorded = (0..acked.recorded).filter(|i| !PAUSE.contains(i)).count() as u32;
    assert!(
        kept <= acked_recorded + 1,
        "{ctx}: {kept} records, more than recorded {acked:?}"
    );
    assert!(
        kept + FLUSH_EVERY_S + 1 >= acked_recorded,
        "{ctx}: {kept} records kept, {acked:?}"
    );
    if acked.stopped {
        assert_eq!(kept, SAMPLES - PAUSE.len() as u32, "{ctx}");
    }
}

#[test]
fn a_ride_survives_any_power_cut() {
    let mut formatted = Flash::new();
    Filesystem::format(&mut formatted).unwrap();
    let mut cut = 1;
    let mut nested = 0;
    loop {
        let mut flash = formatted.clone();
        flash.cut_after(cut);
        let mut acked = Acked::default();
        let done = {
            let mut alloc = Allocation::new();
            let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
            ride(&fs, &mut acked).is_ok()
        };
        if done {
            break;
        }
        assert!(flash.is_off(), "cut {cut}: failed without a power cut");
        flash.power_on();
        let ctx = format!("cut at op {cut}");

        // the recovery itself is cut at each of its operations (every 5th first cut, to keep the test short)
        if cut % 5 == 0 {
            let mut second = 1;
            loop {
                let mut again = flash.clone();
                again.cut_after(second);
                if mount_and_recover(&mut again).is_ok() {
                    break;
                }
                assert!(
                    again.is_off(),
                    "{ctx}, recovery cut {second}: failed without a cut"
                );
                again.power_on();
                mount_and_recover(&mut again)
                    .unwrap_or_else(|e| panic!("{ctx}, recovery cut {second}: {e:?}"));
                check_after(&mut again, &acked, &format!("{ctx}, recovery cut {second}"));
                second += 1;
                nested += 1;
            }
        }
        mount_and_recover(&mut flash).unwrap_or_else(|e| panic!("{ctx}: recovery {e:?}"));
        check_after(&mut flash, &acked, &ctx);
        cut += 1;
    }
    println!(
        "ride: {} flash operations, each cut; {nested} cuts during recoveries",
        cut - 1
    );
    assert!(cut > 50, "suspiciously few operations: {cut}");
}

#[test]
fn odometer() {
    let mut o = Odometer::default();
    assert_eq!(o.update(100, Some(8_000)), 0); // first speed: nothing to integrate yet
    assert_eq!(o.update(101, Some(8_000)), 800);
    assert_eq!(o.update(102, Some(300)), 800); // GNSS noise at rest
    assert_eq!(o.update(103, None), 800); // no fix
    assert_eq!(o.update(104, Some(10_000)), 800 + 2 * 1000); // 2 s since the last speed
    assert_eq!(o.update(160, Some(10_000)), 2800 + 5 * 1000); // long gap: counted 5 s
}

#[test]
fn epoch_clock_without_fix() {
    // RTC ticking 0.4 s after the epochs: at each epoch it still shows the previous second half of the time.
    let mut c = EpochClock::default();
    let rtc = [1000, 1000, 1001, 1003, 1003, 1004];
    let got: Vec<_> = (0..6)
        .map(|i| c.next(36_000 + i, None, Some(rtc[i as usize])))
        .collect();
    assert_eq!(
        got,
        [
            Some(1000),
            Some(1001),
            Some(1002),
            Some(1003),
            Some(1004),
            Some(1005)
        ]
    );
}

#[test]
fn epoch_clock_follows_rtc_and_gnss() {
    let mut c = EpochClock::default();
    assert_eq!(c.next(10, None, None), None); // no time at all
    assert_eq!(c.next(11, None, Some(500)), Some(500));
    assert_eq!(c.next(12, None, Some(510)), Some(510)); // RTC ahead (set meanwhile): follow it
    assert_eq!(c.next(13, Some(511), Some(510)), Some(511)); // valid fix: GNSS time
    assert_eq!(c.next(14, Some(511), Some(511)), None); // repeated second: dropped, never backwards
    assert_eq!(c.next(15, Some(513), None), Some(513));
    assert_eq!(c.next(0, None, None), None); // midnight wrap of a receiver restart: step 86_385 s ignored
    assert_eq!(c.next(1, None, None), Some(514));
}

#[test]
fn ascent_from_the_pressure_only() {
    // 1 h flat: the pressure has the sensor noise (0.8 Pa ≈ 7 cm, rounded to 1 Pa), the altitude jumps by ±10 m
    // (a GNSS altitude, or a filtered altitude whose bias is being corrected): no ascent, no descent
    let mut s = Summary::default();
    s.event(&fit::Event {
        timestamp: t0(),
        kind: fit::EventType::Start,
    });
    let mut x = 1u32;
    for i in 0..3600 {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise_pa = (x >> 30) as i32 - 1; // -1, 0, 1, 2
        s.record(&Record {
            timestamp: t0() + i,
            altitude_dm: Some(2_500 + if i % 120 < 60 { 100 } else { -100 }),
            pressure_pa: Some((pressure(2_500) as i32 + noise_pa) as u32),
            ..Default::default()
        });
    }
    let t = s.totals();
    assert_eq!((t.ascent_m, t.descent_m), (0, 0));

    // without a pressure, the altitude is not counted
    let mut s = Summary::default();
    for i in 0..100 {
        s.record(&Record {
            timestamp: t0() + i,
            altitude_dm: Some(1_000 + i as i32 * 10),
            ..Default::default()
        });
    }
    assert_eq!(s.totals().ascent_m, 0);
}

/// Event types of a FIT file, in order.
fn events(file: &[u8]) -> Vec<String> {
    fitparser::from_bytes(file)
        .unwrap()
        .iter()
        .filter(|m| m.kind() == MesgNum::Event)
        .map(|m| {
            format!(
                "{}",
                m.fields()
                    .iter()
                    .find(|f| f.name() == "event_type")
                    .unwrap()
                    .value()
            )
        })
        .collect()
}

fn session(file: &[u8]) -> FitDataRecord {
    let msgs = fitparser::from_bytes(file).unwrap();
    let sessions: Vec<_> = msgs
        .into_iter()
        .filter(|m| m.kind() == MesgNum::Session)
        .collect();
    assert_eq!(sessions.len(), 1, "one session");
    sessions.into_iter().next().unwrap()
}

fn records(file: &[u8]) -> usize {
    fitparser::from_bytes(file)
        .unwrap()
        .iter()
        .filter(|m| m.kind() == MesgNum::Record)
        .count()
}

/// Sample `i` of the part resumed at `at` (seconds after t0), the distance going on from `from` metres.
fn later(at: u32, from: u32, i: u32) -> Record {
    Record {
        timestamp: t0() + at + i,
        distance_cm: Some((from + i * 8) * 100),
        ..sample(i)
    }
}

#[test]
fn an_interrupted_ride_is_reopened_paused() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    let mut r = Recorder::start(&fs, ID, t0(), 42).unwrap();
    for i in 0..30 {
        r.record(&fs, &sample(i)).unwrap();
    }
    r.flush(&fs).unwrap();
    // the power goes while the timer runs: nothing more is written (the recorder is left as it is)
    let _ = r;
    assert_eq!(interrupted(&fs).unwrap(), Some(ID));
    let mut r = Recorder::reopen(&fs, ID).unwrap();
    assert!(!r.summary().running(), "reopened paused");
    assert_eq!(r.summary().records, 30);
    // back on an hour later
    r.resume(&fs, t0() + 3600).unwrap();
    for i in 0..20 {
        r.record(&fs, &later(3600, 29 * 8, i)).unwrap();
    }
    r.stop(&fs, t0() + 3619, 7200).unwrap();
    let file = read_fit(&fs);
    assert_eq!(events(&file), ["start", "stop", "start", "stop_all"]);
    let s = session(&file);
    // the timer stopped at the last record on flash; the hour switched off does not count
    assert_eq!(num(&s, "total_timer_time"), 29.0 + 19.0);
    assert_eq!(num(&s, "total_elapsed_time"), 3619.0);
    assert_eq!(records(&file), 50);
}

#[test]
fn a_finished_ride_is_resumed() {
    let mut flash = Flash::new();
    Filesystem::format(&mut flash).unwrap();
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
    ride(&fs, &mut Acked::default()).unwrap();
    assert_eq!(last_finished(&fs).unwrap(), Some(ID));
    let mut r = Recorder::reopen_finished(&fs, ID).unwrap();
    assert!(!r.summary().running(), "reopened paused");
    assert_eq!(last_finished(&fs).unwrap(), None);
    r.resume(&fs, t0() + 200).unwrap();
    for i in 0..10 {
        r.record(&fs, &later(200, (SAMPLES - 1) * 8, i)).unwrap();
    }
    let totals = r.stop(&fs, t0() + 209, 7200).unwrap();
    let file = read_fit(&fs);
    // the ride had been stopped while running: reopening stopped its timer where it had ended
    assert_eq!(
        events(&file),
        ["start", "stop", "start", "stop", "start", "stop_all"]
    );
    let s = session(&file);
    let first = (SAMPLES - 1 - PAUSE.len() as u32) as f64;
    assert_eq!(num(&s, "total_timer_time"), first + 9.0);
    assert_eq!(num(&s, "total_elapsed_time"), 209.0);
    assert_eq!(totals.timer_ms, (first as u32 + 9) * 1000);
    assert_eq!(records(&file), SAMPLES as usize - PAUSE.len() + 10);
}

/// What the resumed part was told had been done.
#[derive(Debug, Default)]
struct AckedResume {
    reopened: bool,
    recorded: u32,
    stopped: bool,
}

const MORE: u32 = 30;

fn resume_finished(fs: &Filesystem<'_, Flash>, acked: &mut AckedResume) -> Result<()> {
    let mut r = Recorder::reopen_finished(fs, ID)?;
    acked.reopened = true;
    r.resume(fs, t0() + 200)?;
    for i in 0..MORE {
        r.record(fs, &later(200, (SAMPLES - 1) * 8, i))?;
        acked.recorded += 1;
    }
    r.stop(fs, t0() + 200 + MORE - 1, 7200)?;
    acked.stopped = true;
    Ok(())
}

/// The firmware's boot: the newest interrupted ride is reopened (here closed at once, to check it), the others
/// closed.
fn boot(flash: &mut Flash) -> Result<()> {
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, flash)?;
    let keep = interrupted(&fs)?;
    recover(&fs, 7200, keep, |_| {})?;
    if let Some(id) = keep {
        Recorder::reopen(&fs, id)?.stop(&fs, 0, 7200)?;
    }
    Ok(())
}

/// After a cut while resuming and a boot: one finished ride, whole, with every resumed record acknowledged but
/// the RAM buffer.
fn check_resumed(flash: &mut Flash, acked: &AckedResume, ctx: &str) {
    let mut alloc = Allocation::new();
    let fs = Filesystem::mount(&mut alloc, flash).unwrap_or_else(|e| panic!("{ctx}: mount {e:?}"));
    let mut unfinished = 0;
    ride_store::unfinished(&fs, |_, _| unfinished += 1).unwrap();
    assert_eq!(unfinished, 0, "{ctx}: still unfinished");
    let file = read_fit(&fs);
    fitparser::from_bytes(&file).unwrap_or_else(|e| panic!("{ctx}: undecodable: {e}"));
    session(&file);
    let first = SAMPLES - PAUSE.len() as u32;
    let kept = records(&file) as u32 - first;
    assert!(
        kept <= acked.recorded + 1,
        "{ctx}: {kept} records, {acked:?}"
    );
    assert!(
        kept + FLUSH_EVERY_S + 1 >= acked.recorded,
        "{ctx}: {kept} records kept, {acked:?}"
    );
    if acked.stopped {
        assert_eq!(kept, MORE, "{ctx}");
    }
}

#[test]
fn a_resumed_ride_survives_any_power_cut() {
    let mut base = Flash::new();
    Filesystem::format(&mut base).unwrap();
    {
        let mut alloc = Allocation::new();
        let fs = Filesystem::mount(&mut alloc, &mut base).unwrap();
        ride(&fs, &mut Acked::default()).unwrap();
    }
    let mut cut = 1;
    let mut nested = 0;
    loop {
        let mut flash = base.clone();
        flash.cut_after(cut);
        let mut acked = AckedResume::default();
        let done = {
            let mut alloc = Allocation::new();
            let fs = Filesystem::mount(&mut alloc, &mut flash).unwrap();
            resume_finished(&fs, &mut acked).is_ok()
        };
        if done {
            break;
        }
        assert!(flash.is_off(), "cut {cut}: failed without a power cut");
        flash.power_on();
        let ctx = format!("cut at op {cut}");
        // the boot is cut too, at each of its operations, every 3rd first cut
        if cut % 3 == 0 {
            let mut second = 1;
            loop {
                let mut again = flash.clone();
                again.cut_after(second);
                if boot(&mut again).is_ok() {
                    break;
                }
                assert!(
                    again.is_off(),
                    "{ctx}, boot cut {second}: failed without a cut"
                );
                again.power_on();
                boot(&mut again).unwrap_or_else(|e| panic!("{ctx}, boot cut {second}: {e:?}"));
                check_resumed(&mut again, &acked, &format!("{ctx}, boot cut {second}"));
                second += 1;
                nested += 1;
            }
        }
        boot(&mut flash).unwrap_or_else(|e| panic!("{ctx}: boot {e:?}"));
        check_resumed(&mut flash, &acked, &ctx);
        cut += 1;
    }
    println!(
        "resume: {} flash operations, each cut; {nested} cuts during boots",
        cut - 1
    );
    assert!(cut > 10, "suspiciously few operations: {cut}");
}
