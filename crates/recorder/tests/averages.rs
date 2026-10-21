//! Average power and heart rate of the ride: over the records that have one, zeros included (coasting).

use fit::Record;
use recorder::Summary;

#[test]
fn averages_over_the_records_that_have_one() {
    let mut s = Summary::default();
    assert_eq!((s.avg_power_w(), s.avg_heart_rate()), (None, None));
    let samples = [(Some(200), Some(140)), (Some(0), None), (None, Some(143)), (Some(101), Some(150))];
    for (i, (power_w, heart_rate)) in samples.into_iter().enumerate() {
        s.record(&Record { timestamp: 1_000 + i as u32, power_w, heart_rate, ..Default::default() });
    }
    // (200 + 0 + 101) / 3, (140 + 143 + 150) / 3: rounded
    assert_eq!(s.avg_power_w(), Some(100));
    assert_eq!(s.avg_heart_rate(), Some(144));
}
