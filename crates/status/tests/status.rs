//! The state as the nRF and the GNSS tasks write it, and the time now that the screens and the rides take from it.

use nrf_link::{Message, RtcTime};
use status::{FIX_MAX_AGE_MS, Status};

const RTC: RtcTime = RtcTime { year: 2026, month: 10, day: 20, hour: 9, minute: 30, second: 0 };

/// A valid fix at 09:31:00 on the RTC's day.
fn fix() -> gnss::Fix {
    let mut f = gnss::Fix::default();
    f.valid = true;
    f.time = Some(gnss::Time { hour: 9, minute: 31, second: 0, millis: 0 });
    f.date = Some(gnss::Date { year: 2026, month: 10, day: 20 });
    f.lat_e7 = Some(450_000_000);
    f.lon_e7 = Some(30_000_000);
    f
}

#[test]
fn the_time_is_the_gnss_one_with_a_valid_fix_else_the_rtc() {
    let mut s = Status::new();
    assert_eq!(s.now_unix(), None);
    s.update(Message::Rtc(RTC), 0);
    assert_eq!(s.now_unix(), Some(RTC.unix() as u32));
    s.set_fix(fix(), 0);
    assert_eq!(s.now_unix(), Some(RTC.unix() as u32 + 60));
    // without a fix the receiver's time can be off by tens of seconds
    let mut searching = fix();
    searching.valid = false;
    s.set_fix(searching, 0);
    assert_eq!(s.now_unix(), Some(RTC.unix() as u32));
}

#[test]
fn an_rtc_never_set_is_no_time() {
    let mut s = Status::new();
    s.update(Message::Rtc(RtcTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 5 }), 0);
    assert_eq!(s.now_unix(), None);
}

/// 2026-10-20: the receiver went silent during a ride; its last fix stayed valid, the clock stopped at its time.
#[test]
fn the_fix_of_a_silent_receiver_expires() {
    let mut s = Status::new();
    s.update(Message::Rtc(RTC), 0);
    s.set_fix(fix(), 10_000);
    assert!(!s.expire_fix(10_000 + FIX_MAX_AGE_MS - 1));
    assert_eq!(s.gnss_unix(), Some(RTC.unix() as u32 + 60));
    assert!(s.expire_fix(10_000 + FIX_MAX_AGE_MS));
    assert!(!s.fix.unwrap().valid);
    assert_eq!(s.now_unix(), Some(RTC.unix() as u32));
    // once only
    assert!(!s.expire_fix(20_000));
    // a new epoch is valid again
    s.set_fix(fix(), 20_000);
    assert!(!s.expire_fix(20_000 + 1_000));
    assert!(s.fix.unwrap().valid);
}

#[test]
fn nrf_messages() {
    let mut s = Status::new();
    s.update(Message::Battery { millivolts: 4_100, percent: 87 }, 0);
    s.update(Message::Status(2), 0);
    s.update(Message::Key { key: 1, action: nrf_link::KeyAction::Click }, 0);
    s.update(Message::Key { key: 2, action: nrf_link::KeyAction::LongPress }, 0);
    s.update(Message::Light { level: 900, param: 600 }, 0);
    s.update(Message::Light { level: 4, param: 3000 }, 0);
    s.update(Message::Light { level: 1, param: 1234 }, 0);
    assert_eq!(s.battery, Some((4_100, 87)));
    assert_eq!(s.charger, Some(2));
    assert_eq!(s.keys, 2);
    assert_eq!(s.last_key, Some((2, nrf_link::KeyAction::LongPress)));
    assert_eq!(s.light, [Some(900), None, Some(4)]);
}

#[test]
fn the_pressures_wait_for_the_trip_log() {
    let mut s = Status::new();
    for i in 0..10u32 {
        s.update(Message::Baro { centi_celsius: 2_000, centi_pascal: 10_130_000 + i }, u64::from(i) * 200);
    }
    // 8 kept until the trip log takes them, the latest in `baro`
    assert_eq!(s.baro_recent_n, 8);
    assert_eq!(s.baro_recent[7], 10_130_007);
    assert_eq!(s.baro, Some((2_000, 10_130_009)));
}
