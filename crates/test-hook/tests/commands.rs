//! The lines as the HIL tests and the tools send them.

use nrf_link::KeyAction;
use test_hook::{Command, SimFix, parse, split_seq, unhex};

#[test]
fn a_sequence_number_is_split_off() {
    assert_eq!(split_seq("12 key 2 click"), (Some(12), "key 2 click"));
    assert_eq!(split_seq(" key 2 click\r"), (None, "key 2 click"));
    // a number alone is a command typed by hand, not a sequence number
    assert_eq!(split_seq("42"), (None, "42"));
}

#[test]
fn keys() {
    assert_eq!(parse("key 2 click"), Some(Command::Key(2, &[KeyAction::Click])));
    assert_eq!(
        parse("key 0 long"),
        Some(Command::Key(0, &[KeyAction::LongPress, KeyAction::LongRelease]))
    );
    assert_eq!(parse("key 3 click"), None);
    assert_eq!(parse("key 1 double"), None);
}

#[test]
fn settings() {
    assert_eq!(parse("set tz UTC0"), Some(Command::Set("tz", "UTC0")));
    assert_eq!(parse("get sound"), Some(Command::Get("sound")));
    assert_eq!(parse("settings ?"), Some(Command::Settings));
    assert_eq!(parse("set tz"), None);
}

#[test]
fn gnss() {
    assert_eq!(parse("gnss cold"), Some(Command::GnssStart(gnss::Start::Cold)));
    assert_eq!(parse("gnss full"), Some(Command::GnssStart(gnss::Start::FullCold)));
    assert_eq!(parse("gnss time off"), Some(Command::GnssTimeAiding(false)));
    assert_eq!(parse("gnss off"), Some(Command::GnssOff(0)));
    assert_eq!(parse("gnss off 100"), Some(Command::GnssOff(100)));
    assert_eq!(parse("gnss off many"), None);
    assert_eq!(parse("gnss tepid"), None);
}

#[test]
fn simulated_position_and_light() {
    let at = |speed_mm_s, course_cdeg| {
        Some(Command::SimFix(Some(SimFix { lat_e7: 450_000_000, lon_e7: -30_000_000, speed_mm_s, course_cdeg })))
    };
    assert_eq!(parse("fix 450000000 -30000000"), at(None, None));
    assert_eq!(parse("fix 450000000 -30000000 5000 9000"), at(Some(5000), Some(9000)));
    assert_eq!(parse("fix off"), Some(Command::SimFix(None)));
    assert_eq!(parse("fix 45.0 3.0"), None);
    assert_eq!(parse("light 120"), Some(Command::SimLight(Some(120))));
    assert_eq!(parse("light off"), Some(Command::SimLight(None)));
    assert_eq!(parse("light ?"), Some(Command::Light));
}

#[test]
fn test_switches() {
    assert_eq!(parse("battery fake 100"), Some(Command::FakeBattery(Some(100))));
    assert_eq!(parse("battery fake 101"), None);
    assert_eq!(parse("battery real"), Some(Command::FakeBattery(None)));
    assert_eq!(parse("power off dry"), Some(Command::DryPowerOff));
    assert_eq!(parse("storage fill 40"), Some(Command::StorageFill(Some(40))));
    assert_eq!(parse("storage unfill"), Some(Command::StorageFill(None)));
    assert_eq!(parse("rtc shift -300"), Some(Command::RtcShift(-300)));
    assert_eq!(parse("ble scan 0"), None);
}

#[test]
fn nrf_frames() {
    assert_eq!(parse("nrf e2 1 2 3"), Some(Command::NrfE2(1, 2, 3)));
    assert_eq!(
        parse("nrf raw 5A E2 01 02 03 04 05 06 07 08"),
        Some(Command::NrfRaw(0x5A, 0xE2, [1, 2, 3, 4, 5, 6, 7, 8]))
    );
    // a byte short
    assert_eq!(parse("nrf raw 5A E2 01 02 03 04 05 06 07"), None);
}

#[test]
fn a_route_line_carries_its_crc() {
    let bytes = [0x4f, 0x43, 0x52, 0x54];
    let line = format!("route data 4f435254{:04x}", route::crc16(&bytes));
    assert_eq!(parse(&line), Some(Command::RouteData("4f435254")));
    // a byte lost on the way
    let damaged = format!("route data 4f4354{:04x}", route::crc16(&bytes));
    assert_eq!(parse(&damaged), Some(Command::RouteDataDamaged));
    assert_eq!(parse("route data 12"), Some(Command::RouteDataDamaged));
    let mut buf = Vec::new();
    assert!(unhex("4f435254", &mut buf));
    assert_eq!(buf, bytes);
    assert!(!unhex("4f4", &mut buf));
    assert!(!unhex("zz", &mut buf));
}
