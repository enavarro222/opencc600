use sensors::Revs;
use sensors::ble::{self, Csc, Power, RevEvent};

#[test]
fn heart_rate_8_and_16_bits() {
    assert_eq!(ble::heart_rate(&[0x00, 142]), Some(142));
    // 16 bits, sensor contact and RR intervals after
    assert_eq!(ble::heart_rate(&[0x17, 0x8F, 0x00, 0x10, 0x00, 0x20, 0x03]), Some(143));
    assert_eq!(ble::heart_rate(&[0x01, 0x2C, 0x01]), None, "300 bpm");
    assert_eq!(ble::heart_rate(&[0x00, 0]), None, "no contact yet");
    assert_eq!(ble::heart_rate(&[0x01, 0x8F]), None, "cut");
    assert_eq!(ble::heart_rate(&[]), None);
}

#[test]
fn speed_and_cadence() {
    // the fake sensor's frame (tools/fake_sensor.py): wheel and crank
    let data = [0x03, 0x10, 0x27, 0x00, 0x00, 0x00, 0x04, 0x2A, 0x00, 0x00, 0x08];
    let want = Csc {
        wheel: Some(RevEvent { revs: 10_000, time: 1024 }),
        crank: Some(RevEvent { revs: 42, time: 2048 }),
    };
    assert_eq!(ble::csc(&data), Some(want));
    // a cadence-only sensor
    assert_eq!(
        ble::csc(&[0x02, 0x2A, 0x00, 0x00, 0x08]),
        Some(Csc { wheel: None, crank: Some(RevEvent { revs: 42, time: 2048 }) })
    );
    assert_eq!(ble::csc(&[0x03, 0x10, 0x27]), None);
}

#[test]
fn power_and_its_optional_fields() {
    assert_eq!(ble::power(&[0x00, 0x00, 0xDC, 0x00]), Some(Power { watts: 220, crank: None }));
    // balance, torque and wheel data before the crank data
    let data = [0x35, 0x00, 0x2C, 0x01, 50, 0x10, 0x00, 1, 0, 0, 0, 0, 0, 0x07, 0x00, 0x00, 0x04];
    assert_eq!(ble::power(&data), Some(Power { watts: 300, crank: Some(RevEvent { revs: 7, time: 1024 }) }));
    // a trainer braking
    assert_eq!(ble::power(&[0x00, 0x00, 0xF6, 0xFF]), Some(Power { watts: 0, crank: None }));
    assert_eq!(ble::power(&[0x20, 0x00, 0xDC, 0x00, 0x07]), None);
}

const CRANK: u32 = u16::MAX as u32;
const WHEEL: u32 = u32::MAX;

#[test]
fn cadence_from_crank_revolutions() {
    let mut r = Revs::default();
    // 90 rpm: a revolution every 2/3 s, the sensor sending every 250 ms
    let mut revs = 0u32;
    let mut time = 0u16;
    r.update(revs, time, CRANK, 0);
    for k in 1..=40u64 {
        // a new revolution at each 2/3 s, the same event repeated in between
        if k % 8 == 0 || k % 8 == 3 || k % 8 == 5 {
            revs += 1;
            time = time.wrapping_add(683);
        }
        r.update(revs, time, CRANK, k * 250);
    }
    assert!((89..=91).contains(&r.rpm().unwrap()), "{:?}", r.rpm());
}

#[test]
fn counters_wrap() {
    let mut r = Revs::default();
    r.update(0xFFFF, 0xFF00, CRANK, 0);
    // 2 revolutions in 1 s across both wraps: 120 rpm
    assert_eq!(r.update(1, 0x0300, CRANK, 1000), 2);
    assert_eq!(r.rpm(), Some(120));
}

#[test]
fn stopped_after_three_seconds() {
    let mut r = Revs::default();
    r.update(10, 0, CRANK, 0);
    assert_eq!(r.rpm(), None, "one event: no rate yet");
    r.update(11, 1024, CRANK, 1000);
    assert_eq!(r.rpm(), Some(60));
    // the same event repeated
    r.update(11, 1024, CRANK, 3000);
    assert_eq!(r.rpm(), Some(60), "a slow revolution still counts");
    r.update(11, 1024, CRANK, 4100);
    assert_eq!(r.rpm(), Some(0));
}

#[test]
fn wheel_speed() {
    let mut r = Revs::default();
    r.update(1000, 0, WHEEL, 0);
    // 30 km/h on a 2105 mm wheel: 3.96 revolutions a second
    r.update(1004, 1034, WHEEL, 1000);
    let mm_s = r.speed_mm_s(2105).unwrap();
    assert!((8_300..=8_400).contains(&mm_s), "{mm_s}");
}

#[test]
fn a_jump_is_not_a_rate() {
    let mut r = Revs::default();
    r.update(5, 0, CRANK, 0);
    // the sensor reset its counter
    assert_eq!(r.update(30_000, 1024, CRANK, 1000), 0);
    assert_eq!(r.rpm(), None);
    assert_eq!(r.update(30_001, 2048, CRANK, 2000), 1);
    assert_eq!(r.rpm(), Some(60));
}

#[test]
fn power_over_the_last_3_seconds() {
    use sensors::Avg3s;
    let mut a = Avg3s::default();
    assert_eq!(a.get(0), None);
    // second 0: 100 and 200 (avg 150), second 1: 300, second 2: 400
    a.push(Some(100), 100);
    a.push(Some(200), 600);
    a.push(Some(300), 1_200);
    a.push(Some(400), 2_300);
    assert_eq!(a.get(2_500), Some(283));
    // second 3: second 0 leaves
    a.push(Some(500), 3_100);
    assert_eq!(a.get(3_200), Some(400));
    // nothing more: gone after 3 s
    a.push(None, 5_000);
    assert_eq!(a.get(5_500), Some(500));
    assert_eq!(a.get(6_000), None);
}
