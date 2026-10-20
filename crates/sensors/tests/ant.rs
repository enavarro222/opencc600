//! ANT+ pages and channels. Pages from c606-oss `docs/HARDWARE.md` (seen on a C606) and the ANT+ device profiles;
//! the channel behaviour is the one c606-oss found on the hardware.

use sensors::ant::{self, Ant, Found, Id, Link, Request, Values};
use sensors::ble::{Csc, RevEvent};

const HR: Id = Id { device_type: ant::HEART_RATE, number: 12345, transmission: 1 };
const SPEED: Id = Id { device_type: ant::SPEED, number: 4242, transmission: 1 };
const PM: Id = Id { device_type: ant::POWER, number: 777, transmission: 5 };
/// Opens allowed from 8 s, as c606-oss.
const OPEN_FROM: u64 = 8_000;

fn hr_page(bpm: u8, beat_time: u16, count: u8) -> [u8; 8] {
    let [lo, hi] = beat_time.to_le_bytes();
    [0x84, 0x00, 0x00, 0x00, lo, hi, count, bpm]
}

/// A speed sensor page (123): reserved bytes 0xFF, event time and revolutions.
fn speed_page(time: u16, revs: u16) -> [u8; 8] {
    let ([t0, t1], [r0, r1]) = (time.to_le_bytes(), revs.to_le_bytes());
    [0xFF, 0xFF, 0xFF, 0xFF, t0, t1, r0, r1]
}

#[test]
fn heart_rate_page() {
    assert_eq!(ant::heart_rate(&hr_page(72, 0x2400, 5)), Some(72));
    assert_eq!(ant::heart_rate(&hr_page(0, 0, 0)), None, "no contact");
}

#[test]
fn speed_and_cadence_pages() {
    let p = [0x00, 0x04, 0x2A, 0x00, 0x00, 0x08, 0x10, 0x27];
    assert_eq!(
        ant::speed_cadence(ant::SPEED_CADENCE, &p),
        Csc {
            crank: Some(RevEvent { time: 1024, revs: 42 }),
            wheel: Some(RevEvent { time: 2048, revs: 10_000 }),
        }
    );
    assert_eq!(
        ant::speed_cadence(ant::CADENCE, &speed_page(1024, 42)),
        Csc { wheel: None, crank: Some(RevEvent { time: 1024, revs: 42 }) }
    );
    assert_eq!(
        ant::speed_cadence(ant::SPEED, &speed_page(1024, 42)),
        Csc { wheel: Some(RevEvent { time: 1024, revs: 42 }), crank: None }
    );
    assert_eq!(ant::speed_cadence(ant::HEART_RATE, &p), Csc::default());
}

#[test]
fn power_page() {
    // page 0x10: event count, pedal balance, cadence 90, accumulated power, instant power 250 W
    let p = [0x10, 0x05, 0xFF, 90, 0x00, 0x10, 250, 0];
    assert_eq!(ant::power(&p), Some(ant::Power { watts: 250, cadence_rpm: Some(90) }));
    let no_cadence = [0x10, 0x05, 0xFF, 0xFF, 0x00, 0x10, 0x2C, 0x01];
    assert_eq!(ant::power(&no_cadence), Some(ant::Power { watts: 300, cadence_rpm: None }));
    assert_eq!(ant::power(&[0x12, 0, 0, 0, 0, 0, 0, 0]), None, "torque page");
}

#[test]
fn roles() {
    assert_eq!(ant::role(ant::HEART_RATE), Some(0));
    for t in [ant::SPEED_CADENCE, ant::CADENCE, ant::SPEED] {
        assert_eq!(ant::role(t), Some(1));
    }
    assert_eq!(ant::role(ant::POWER), Some(2));
    assert_eq!(ant::role(17), None, "a trainer");
}

/// Polls every 10 ms until `until_ms`, the requests with their time.
fn requests(a: &mut Ant, from_ms: u64, until_ms: u64) -> Vec<(u64, Request)> {
    (from_ms..until_ms).step_by(10).filter_map(|t| a.poll(t).map(|r| (t, r))).collect()
}

#[test]
fn channels_open_after_the_first_seconds_one_at_a_time() {
    let mut a = Ant::new(OPEN_FROM, 2105);
    a.pair([Some(HR), Some(SPEED), Some(PM)]);
    assert_eq!(requests(&mut a, 0, OPEN_FROM), vec![], "not in the first seconds");
    let got = requests(&mut a, OPEN_FROM, OPEN_FROM + 1_000);
    assert_eq!(
        got,
        vec![
            (OPEN_FROM, Request::Open(HR)),
            (OPEN_FROM + 100, Request::Open(SPEED)),
            (OPEN_FROM + 200, Request::Open(PM)),
        ]
    );
    // the radio searches: nothing more asked while it does
    for id in [HR, SPEED, PM] {
        a.channel_state(id.device_type, 4, OPEN_FROM + 1_000);
    }
    assert_eq!(requests(&mut a, OPEN_FROM + 1_000, OPEN_FROM + 60_000), vec![]);
    assert_eq!(a.link(0, OPEN_FROM + 60_000), Link::Searching);
}

#[test]
fn a_connected_sensor_gives_its_values() {
    let mut a = Ant::new(0, 2000);
    a.pair([Some(HR), Some(SPEED), Some(PM)]);
    let _ = requests(&mut a, 0, 1_000);
    a.channel_state(ant::HEART_RATE, 3, 1_000);
    a.page(ant::HEART_RATE, &hr_page(142, 0, 1), 1_000);
    // one wheel turn a second: 2 m/s
    for s in 0..4u16 {
        a.page(ant::SPEED, &speed_page(s * 1024, 100 + s), 1_000 + s as u64 * 1_000);
    }
    a.page(PM.device_type, &[0x10, 1, 0xFF, 85, 0, 0, 200, 0], 4_000);
    let now = 4_000;
    assert_eq!(
        a.values(now),
        Values { heart_rate: Some(142), cadence_rpm: Some(85), speed_mm_s: Some(2_000), power_w: Some(200) }
    );
    assert_eq!(a.link(0, now), Link::Connected);
    assert_eq!(a.link(0, 1_000 + Ant::STALE_MS), Link::Searching, "no page for 5 s");
    assert_eq!(a.link(1, now), Link::Connected);
    assert_eq!(a.link(2, now), Link::Connected);
    assert_eq!(a.values(now + Ant::STALE_MS), Values::default(), "all gone");
    assert_eq!(a.link_of(SPEED, now), Link::Connected);
    assert_eq!(a.link_of(Id { number: 1, ..SPEED }, now), Link::None);
}

#[test]
fn a_channel_kept_by_the_radio_is_not_opened_again() {
    // the ESP32 restarted, the radio kept the channel and reports it connected before the opens
    let mut a = Ant::new(OPEN_FROM, 2105);
    a.pair([Some(HR), None, None]);
    a.channel_state(ant::HEART_RATE, 3, 5_000);
    a.page(ant::HEART_RATE, &hr_page(130, 0, 1), 5_200);
    assert_eq!(requests(&mut a, 5_200, 20_000), vec![]);
    assert_eq!(a.values(9_000).heart_rate, Some(130));
}

#[test]
fn the_periodic_not_connected_report_keeps_a_live_channel() {
    let mut a = Ant::new(0, 2105);
    a.pair([Some(HR), None, None]);
    let _ = requests(&mut a, 0, 100);
    a.channel_state(ant::HEART_RATE, 3, 1_000);
    let mut sent = vec![];
    for t in (1_000..60_000).step_by(250) {
        a.page(ant::HEART_RATE, &hr_page(120, t as u16, 1), t);
        if t % 5_000 == 0 {
            a.channel_state(ant::HEART_RATE, 4, t);
        }
        sent.extend(a.poll(t));
    }
    assert_eq!(sent, vec![], "never reopened");
    assert_eq!(a.link(0, 60_000), Link::Connected);
}

#[test]
fn a_search_timeout_opens_the_channel_again() {
    let mut a = Ant::new(0, 2105);
    a.pair([Some(HR), None, None]);
    assert_eq!(a.poll(0), Some(Request::Open(HR)));
    a.channel_state(ant::HEART_RATE, 4, 500);
    a.channel_state(ant::HEART_RATE, 5, 25_000);
    assert_eq!(a.poll(25_010), Some(Request::Open(HR)));
}

#[test]
fn a_long_search_is_asked_again() {
    // the CC600's radio: "not connected" a few seconds after the open, then nothing (no search timeout)
    let mut a = Ant::new(0, 2105);
    a.pair([Some(HR), None, None]);
    assert_eq!(a.poll(0), Some(Request::Open(HR)));
    a.channel_state(ant::HEART_RATE, 4, 4_000);
    assert_eq!(requests(&mut a, 10, 4_000 + Ant::SEARCH_AGAIN_MS), vec![]);
    assert_eq!(a.poll(4_000 + Ant::SEARCH_AGAIN_MS), Some(Request::Open(HR)));
}

#[test]
fn an_open_never_answered_is_asked_again() {
    let mut a = Ant::new(0, 2105);
    a.pair([Some(HR), None, None]);
    assert_eq!(a.poll(0), Some(Request::Open(HR)));
    assert_eq!(requests(&mut a, 10, Ant::ASK_AGAIN_MS), vec![]);
    assert_eq!(a.poll(Ant::ASK_AGAIN_MS), Some(Request::Open(HR)));
}

#[test]
fn a_channel_silent_for_long_is_opened_again() {
    let mut a = Ant::new(0, 2105);
    a.pair([Some(HR), None, None]);
    let _ = a.poll(0);
    a.page(ant::HEART_RATE, &hr_page(120, 0, 1), 1_000);
    // the radio still says connected now and then, no page
    for t in [5_000, 10_000, 20_000] {
        a.channel_state(ant::HEART_RATE, 3, t);
    }
    assert_eq!(requests(&mut a, 1_000, 1_000 + Ant::LOST_MS), vec![]);
    assert_eq!(a.poll(1_000 + Ant::LOST_MS), Some(Request::Open(HR)));
}

#[test]
fn another_sensor_paired_closes_the_old_channel_first() {
    let mut a = Ant::new(0, 2105);
    a.pair([Some(HR), None, None]);
    let _ = a.poll(0);
    a.channel_state(ant::HEART_RATE, 3, 500);
    let other = Id { number: 999, ..HR };
    a.pair([Some(other), None, None]);
    assert_eq!(
        requests(&mut a, 1_000, 2_000),
        vec![(1_000, Request::Close(ant::HEART_RATE)), (1_100, Request::Open(other))]
    );
    // forgotten: closed, nothing opened
    a.pair([None, None, None]);
    assert_eq!(requests(&mut a, 2_000, 3_000), vec![(2_000, Request::Close(ant::HEART_RATE))]);
    assert_eq!(a.link(0, 3_000), Link::None);
}

#[test]
fn a_sensor_never_opened_is_not_closed() {
    // paired then replaced before the first opens (at boot): closing at boot left the radio silent
    let mut a = Ant::new(OPEN_FROM, 2105);
    a.pair([Some(HR), None, None]);
    a.pair([None, None, None]);
    assert_eq!(requests(&mut a, 0, 20_000), vec![]);
}

#[test]
fn pages_of_other_sensors_are_ignored() {
    let mut a = Ant::new(0, 2105);
    a.pair([None, Some(SPEED), None]);
    a.page(ant::HEART_RATE, &hr_page(150, 0, 1), 100);
    a.page(ant::CADENCE, &speed_page(1024, 3), 100);
    assert_eq!(a.values(200), Values::default());
}

#[test]
fn search_results_by_role_closest_first() {
    let mut a = Ant::new(0, 2105);
    a.search_started();
    let far = Id { number: 1, ..HR };
    a.heard(far, -80);
    a.heard(HR, -60);
    a.heard(SPEED, -40);
    a.heard(Id { device_type: 17, number: 5, transmission: 1 }, -30);
    a.heard(Id { number: 0, ..PM }, -30);
    a.heard(far, -50);
    let hr: Vec<Found> = a.found(0).collect();
    assert_eq!(hr, vec![Found { id: far, rssi_dbm: -50 }, Found { id: HR, rssi_dbm: -60 }]);
    assert_eq!(a.found(1).collect::<Vec<_>>(), vec![Found { id: SPEED, rssi_dbm: -40 }]);
    assert_eq!(a.found(2).count(), 0, "a trainer and a number 0 are left out");
    // full: the weakest goes for a closer one
    for n in 10..20 {
        a.heard(Id { number: n, ..PM }, -(n as i8) - 40);
    }
    assert_eq!(a.found(2).count() + a.found(0).count() + a.found(1).count(), ant::FOUND_MAX);
    assert_eq!(a.found(2).next().map(|f| f.rssi_dbm), Some(-50));
    a.search_started();
    assert_eq!(a.found(0).count(), 0);
}
