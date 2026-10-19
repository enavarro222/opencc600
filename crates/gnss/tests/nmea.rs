//! Parsing checked against the anonymised CC600 capture (tests/fixtures/README.md).

use gnss::{
    Aiding, Epoch, Fix, LineReader, OUTPUT_SETUP, aiding_request, is_started, is_turned_off, pair_ack, write_command,
};

const CAPTURE: &[u8] = include_bytes!("fixtures/ag3352q_10s.nmea");

fn replay(data: &[u8], chunk: usize) -> (Vec<Fix>, gnss::Stats) {
    let (mut reader, mut fix, mut epochs) = (LineReader::new(), Fix::default(), vec![]);
    for part in data.chunks(chunk) {
        reader.feed(part, |body| {
            if fix.update(body) == Epoch::Complete {
                epochs.push(fix);
            }
        });
    }
    (epochs, reader.stats)
}

#[test]
fn capture_epochs() {
    let (epochs, stats) = replay(CAPTURE, 1024);
    assert_eq!(
        stats.bad_checksum + stats.cut + stats.too_long + stats.malformed,
        0,
        "{stats:?}"
    );
    assert_eq!(stats.sentences, 253);
    // the capture starts at an RMC without its GGA: 9 complete epochs out of 10
    assert_eq!(epochs.len(), 9);
    let last = epochs.last().unwrap();
    assert!(last.valid);
    assert_eq!((last.quality, last.sats_used), (1, 18));
    assert_eq!(
        last.date,
        Some(gnss::Date {
            year: 2026,
            month: 10,
            day: 19
        })
    );
    // fake place of the fixture: 45°00.00108'N 6°00.00063'E at epoch 9
    assert_eq!(last.lat_e7, Some(450_000_000 + 108 * 100 / 60));
    assert_eq!(last.lon_e7, Some(60_000_000 + 63 * 100 / 60));
    // the fixture numbers its epochs at RMC, and GGA comes before RMC: this GGA carries epoch 8's 350.8 m
    assert_eq!(last.alt_dm, Some(3508));
    assert!(last.sats_in_view >= 40, "{}", last.sats_in_view);
    assert!(last.hdop_c.unwrap() < 100);
    // one epoch per second
    for pair in epochs.windows(2) {
        let (a, b) = (pair[0].time.unwrap(), pair[1].time.unwrap());
        assert_eq!((b.second + 60 - a.second) % 60, 1);
    }
}

#[test]
fn split_anywhere() {
    let (whole, _) = replay(CAPTURE, CAPTURE.len());
    for chunk in [1, 7, 64, 333] {
        assert_eq!(replay(CAPTURE, chunk).0, whole, "chunk {chunk}");
    }
}

#[test]
fn rmc_fields() {
    let mut fix = Fix::default();
    fix.update("GNRMC,123519.250,A,4807.03800,N,01131.00000,W,10.0,084.40,230394,,,A,V");
    assert_eq!(
        fix.time,
        Some(gnss::Time {
            hour: 12,
            minute: 35,
            second: 19,
            millis: 250
        })
    );
    assert_eq!(fix.lat_e7, Some(481_173_000)); // 48° + 7.038' / 60
    assert_eq!(fix.lon_e7, Some(-115_166_666)); // 11° + 31' / 60, west
    assert_eq!(fix.speed_mm_s, Some(5144)); // 10 kn
    assert_eq!(fix.course_cdeg, Some(8440));
    // no fix: the position is cleared, the time is kept
    fix.update("GNRMC,123520.000,V,,,,,,,230394,,,N,V");
    assert!(!fix.valid);
    assert_eq!((fix.lat_e7, fix.speed_mm_s), (None, None));
    assert_eq!(fix.time.unwrap().second, 20);
}

#[test]
fn garbage_and_errors() {
    let mut reader = LineReader::new();
    let mut n = 0;
    let long = format!("$PAIRSAT,{}*00\r\n", "1,".repeat(100));
    let mut data = b"\x00\xff$GNGGA,1*00\r\n$GNRMC,cut".to_vec(); // bad checksum, then a line cut by the next $
    data.extend_from_slice(long.as_bytes());
    data.extend_from_slice(b"no checksum\r\n$PAIR001,062,0*3F\r\n");
    reader.feed(&data, |_| n += 1);
    let s = reader.stats;
    assert_eq!((s.bad_checksum, s.cut, s.too_long), (1, 1, 1), "{s:?}");
    assert_eq!(n, 1);
}

#[test]
fn pair_commands() {
    let mut out = [0u8; 32];
    let n = write_command("PAIR062,0,1", &mut out).unwrap();
    let mut seen = vec![];
    LineReader::new().feed(&out[..n], |b| seen.push(b.to_string()));
    assert_eq!(seen, ["PAIR062,0,1"]);
    assert!(out[..n].ends_with(b"\r\n"));
    assert_eq!(pair_ack("PAIR001,062,0"), Some((62, 0)));
    assert_eq!(pair_ack("PAIR002,1"), None);
}

#[test]
fn output_setup() {
    // Each command fits the firmware's buffer and round-trips through the reader.
    for body in OUTPUT_SETUP {
        let mut out = [0u8; 32];
        let n = write_command(body, &mut out).unwrap();
        let mut seen = vec![];
        LineReader::new().feed(&out[..n], |b| seen.push(b.to_string()));
        assert_eq!(seen, [body]);
    }
    // Known checksum (forum list of PAIR commands): `$PAIR062,1,0*3F`.
    let mut out = [0u8; 32];
    let n = write_command(OUTPUT_SETUP[1], &mut out).unwrap();
    assert_eq!(&out[..n], b"$PAIR062,1,0*3F\r\n");

    for off in ["GNGLL,", "GNGSA,A,1", "GNVTG,", "GNZDA,1"] {
        assert!(is_turned_off(off), "{off}");
    }
    for on in ["GNGGA,", "GNRMC,", "GPGSV,", "PAIR001,062,0", "GN"] {
        assert!(!is_turned_off(on), "{on}");
    }
}

#[test]
fn start_messages() {
    // Seen at a receiver start (dump/, 2026-10-19).
    assert_eq!(aiding_request("PAIR010,0,2,2441,122873"), Some(Aiding::Epo(2)));
    assert_eq!(aiding_request("PAIR010,1,-1"), Some(Aiding::Time));
    assert_eq!(aiding_request("PAIR010,2,-1"), Some(Aiding::Position));
    assert_eq!(aiding_request("PAIR001,010,0"), None);
    assert!(is_started("PAIR011,001"));
    assert!(!is_started("PAIR011,002"));
}
