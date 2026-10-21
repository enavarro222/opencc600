//! The session driven as the app drives it: frames cut in BLE-sized writes, files sent with the app's `Sender`.

use companion::{Device, Event, Session};
use protocol_cc600::payload::{ActivityEntry, ReadActivity};
use protocol_cc600::transfer::{Ack, Begin, Kind, Ready, Sender, chunk_header};
use protocol_cc600::{ErrorCode, Frame, Op, crc16_ccitt};

#[derive(Default)]
struct FakeDevice {
    settings: settings::Settings,
    files: Vec<(Kind, Vec<u8>)>,
    /// `max_file` answers this for maps (routes: 64 KB).
    map: Option<Result<u32, ErrorCode>>,
    /// Finished rides: id, file, sent.
    rides: Vec<(u32, Vec<u8>, bool)>,
}

impl Device for FakeDevice {
    fn settings(&self) -> settings::Settings {
        self.settings
    }
    fn set_settings(&mut self, s: settings::Settings) {
        self.settings = s;
    }
    fn max_file(&self, kind: Kind) -> Result<u32, ErrorCode> {
        match kind {
            Kind::Route => Ok(64 * 1024),
            Kind::Map => self.map.unwrap_or(Err(ErrorCode::BadRequest)),
        }
    }
    fn file_received(&mut self, kind: Kind, data: Vec<u8>) -> Result<(), ErrorCode> {
        self.files.push((kind, data));
        Ok(())
    }
    async fn rides(&mut self) -> Result<Vec<ActivityEntry>, ErrorCode> {
        Ok(self
            .rides
            .iter()
            .map(|(id, file, sent)| ActivityEntry {
                id: *id,
                start_time: id - 631_065_600,
                size: file.len() as u32,
                synced: *sent,
            })
            .collect())
    }
    async fn read_ride(&mut self, id: u32, offset: u32, max: u16) -> Result<Vec<u8>, ErrorCode> {
        let (_, file, _) = self
            .rides
            .iter()
            .find(|r| r.0 == id)
            .ok_or(ErrorCode::BadRequest)?;
        let start = (offset as usize).min(file.len());
        Ok(file[start..(start + max as usize).min(file.len())].to_vec())
    }
    async fn ride_sent(&mut self, id: u32) -> Result<(), ErrorCode> {
        let r = self
            .rides
            .iter_mut()
            .find(|r| r.0 == id)
            .ok_or(ErrorCode::BadRequest)?;
        r.2 = true;
        Ok(())
    }
}

/// The fake device never waits: one poll does.
fn block_on<T>(f: impl Future<Output = T>) -> T {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match std::pin::pin!(f).poll(&mut cx) {
        std::task::Poll::Ready(v) => v,
        std::task::Poll::Pending => panic!("the fake device waited"),
    }
}

fn frame(op: Op, seq: u8, payload: &[u8]) -> Vec<u8> {
    let f = Frame::new(op, seq, payload);
    let mut b = vec![0; f.encoded_len()];
    f.encode(&mut b).unwrap();
    b
}

/// The answers as (op, seq, payload).
fn answers(out: &[u8]) -> Vec<(Op, u8, Vec<u8>)> {
    let mut rest = out;
    let mut v = Vec::new();
    while !rest.is_empty() {
        let (f, n) = Frame::decode(rest).unwrap();
        v.push((f.op, f.seq, f.payload.to_vec()));
        rest = &rest[n..];
    }
    v
}

/// Writes `bytes` 20 at a time (the smallest BLE write), as Web Bluetooth does.
fn write(s: &mut Session, d: &mut FakeDevice, bytes: &[u8]) -> Vec<Event> {
    bytes
        .chunks(20)
        .flat_map(|c| block_on(s.on_write(c, d)))
        .collect()
}

fn request(
    s: &mut Session,
    d: &mut FakeDevice,
    op: Op,
    seq: u8,
    payload: &[u8],
) -> Vec<(Op, u8, Vec<u8>)> {
    write(s, d, &frame(op, seq, payload));
    answers(&s.take_output())
}

fn route_file() -> Vec<u8> {
    let mut gpx = String::from("<gpx><trk><name>Test</name><trkseg>");
    // a zigzag: every point kept by the simplification
    for i in 0..1000 {
        let (lat, lon) = (45.0 + i as f64 * 0.0001, 3.0 + (i % 2) as f64 * 0.0003);
        gpx += &format!(
            "<trkpt lat=\"{lat}\" lon=\"{lon}\"><ele>{}</ele></trkpt>",
            200 + i / 4
        );
    }
    gpx += "</trkseg></trk></gpx>";
    route::build(&route::parse_gpx(&gpx).unwrap(), 1.0)
}

/// Sends `data` like the app: chunks of 240 bytes in the window, `cut` after that many chunks (the link drops).
/// Returns the file bytes sent.
fn send(
    s: &mut Session,
    d: &mut FakeDevice,
    kind: Kind,
    data: &[u8],
    cut: Option<usize>,
) -> Result<usize, ErrorCode> {
    let begin = Begin {
        kind,
        size: data.len() as u32,
        crc: crc16_ccitt(data),
    };
    let a = request(s, d, Op::TransferBegin, 1, &begin.to_bytes());
    let (op, _, p) = &a[0];
    if *op == Op::Error {
        return Err(ErrorCode::from_u8(p[0]).unwrap());
    }
    let mut tx = Sender::new(begin.size);
    tx.ready(Ready::from_bytes(p).unwrap());
    let (mut sent, mut chunks) = (0, 0);
    while tx.permille() < 1000 {
        let mut progressed = false;
        while let Some((offset, len)) = tx.next_chunk(240) {
            progressed = true;
            chunks += 1;
            if cut == Some(chunks) {
                s.disconnected();
                return Err(ErrorCode::Busy);
            }
            let mut payload = chunk_header(offset).to_vec();
            payload.extend_from_slice(&data[offset as usize..(offset + len) as usize]);
            sent += len as usize;
            for (op, _, p) in request(s, d, Op::TransferChunk, 2, &payload) {
                match op {
                    Op::TransferAck => tx.ack(Ack::from_bytes(&p).unwrap()),
                    Op::TransferResend => tx.resend(Ack::from_bytes(&p).unwrap()),
                    _ => return Err(ErrorCode::from_u8(p[0]).unwrap()),
                }
            }
        }
        assert!(progressed, "stalled");
    }
    let a = request(s, d, Op::TransferEnd, 3, &[]);
    match &a[0] {
        (Op::Ok, ..) => Ok(sent),
        (_, _, p) => Err(ErrorCode::from_u8(p[0]).unwrap()),
    }
}

#[test]
fn answers_hello_written_in_pieces() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    assert_eq!(
        request(&mut s, &mut d, Op::Hello, 7, &[]),
        [(Op::DeviceInfo, 7, vec![])]
    );
}

#[test]
fn several_frames_in_one_write() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let mut both = frame(Op::Hello, 1, &[]);
    both.extend(frame(Op::Hello, 2, &[]));
    block_on(s.on_write(&both, &mut d));
    let seqs: Vec<u8> = answers(&s.take_output()).iter().map(|a| a.1).collect();
    assert_eq!(seqs, [1, 2]);
}

#[test]
fn a_damaged_frame_is_dropped_and_the_next_one_answered() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let mut bad = frame(Op::Hello, 1, &[]);
    bad[2] ^= 0xFF;
    bad.extend(frame(Op::Hello, 2, &[]));
    block_on(s.on_write(&bad, &mut d));
    assert_eq!(answers(&s.take_output()), [(Op::DeviceInfo, 2, vec![])]);
}

fn with_rides() -> FakeDevice {
    let ride = |id: u32, len: usize| {
        (
            id,
            (0..len).map(|i| (i * 7 + id as usize) as u8).collect(),
            false,
        )
    };
    FakeDevice {
        rides: vec![ride(1_760_000_000, 5000), ride(1_760_100_000, 0)],
        ..Default::default()
    }
}

/// Ride `id` read as Musette does, `max` bytes at a time.
fn download(s: &mut Session, d: &mut FakeDevice, id: u32, max: u16) -> Vec<u8> {
    let mut file = Vec::new();
    loop {
        let r = ReadActivity {
            id,
            offset: file.len() as u32,
            max,
        };
        let a = request(s, d, Op::ReadActivity, 5, &r.to_bytes());
        let [(Op::ActivityChunk, 5, data)] = &a[..] else {
            panic!("{a:?}")
        };
        if data.is_empty() {
            return file;
        }
        file.extend_from_slice(data);
    }
}

fn ack(s: &mut Session, d: &mut FakeDevice, id: u32, crc: u16) -> Vec<(Op, u8, Vec<u8>)> {
    let mut p = id.to_le_bytes().to_vec();
    p.extend(crc.to_le_bytes());
    request(s, d, Op::AckActivity, 6, &p)
}

#[test]
fn lists_the_rides() {
    let (mut s, mut d) = (Session::new(), with_rides());
    let a = request(&mut s, &mut d, Op::ListActivities, 4, &[]);
    let entries: Vec<_> = a[..2]
        .iter()
        .map(|(_, _, p)| ActivityEntry::from_bytes(p).unwrap())
        .collect();
    assert_eq!(entries[0].id, 1_760_000_000);
    assert_eq!(entries[0].size, 5000);
    // 2025-10-09T08:53:20Z
    assert_eq!(entries[0].start_time, 1_128_934_400);
    assert!(!entries[0].synced);
    assert_eq!(entries[1].size, 0);
    assert_eq!(
        a.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(),
        [
            (Op::ActivityEntry, 4),
            (Op::ActivityEntry, 4),
            (Op::EndOfList, 4)
        ]
    );
}

#[test]
fn a_ride_is_read_in_pieces_and_acknowledged() {
    let (mut s, mut d) = (Session::new(), with_rides());
    let file = download(&mut s, &mut d, 1_760_000_000, 512);
    assert_eq!(file, d.rides[0].1);
    assert_eq!(
        ack(&mut s, &mut d, 1_760_000_000, crc16_ccitt(&file)),
        [(Op::Ok, 6, vec![])]
    );
    assert!(d.rides[0].2);
    let a = request(&mut s, &mut d, Op::ListActivities, 4, &[]);
    assert!(ActivityEntry::from_bytes(&a[0].2).unwrap().synced);
}

#[test]
fn a_piece_is_at_most_a_frame() {
    let (mut s, mut d) = (Session::new(), with_rides());
    let r = ReadActivity {
        id: 1_760_000_000,
        offset: 0,
        max: u16::MAX,
    };
    let a = request(&mut s, &mut d, Op::ReadActivity, 5, &r.to_bytes());
    assert_eq!(a[0].2.len(), protocol_cc600::MAX_PAYLOAD);
}

#[test]
fn an_ack_with_a_wrong_crc_is_refused() {
    let (mut s, mut d) = (Session::new(), with_rides());
    let crc = crc16_ccitt(&d.rides[0].1);
    let a = ack(&mut s, &mut d, 1_760_000_000, crc ^ 1);
    assert_eq!(a, [(Op::Error, 6, vec![ErrorCode::BadCrc as u8])]);
    assert!(!d.rides[0].2);
}

#[test]
fn an_ack_without_a_crc_is_taken() {
    // Musette sends 0 for now
    let (mut s, mut d) = (Session::new(), with_rides());
    assert_eq!(ack(&mut s, &mut d, 1_760_000_000, 0), [(Op::Ok, 6, vec![])]);
    assert!(d.rides[0].2);
}

#[test]
fn an_unknown_ride_is_an_error() {
    let (mut s, mut d) = (Session::new(), with_rides());
    let r = ReadActivity {
        id: 3,
        offset: 0,
        max: 512,
    };
    let a = request(&mut s, &mut d, Op::ReadActivity, 5, &r.to_bytes());
    assert_eq!(a, [(Op::Error, 5, vec![ErrorCode::BadRequest as u8])]);
    assert_eq!(
        ack(&mut s, &mut d, 3, 0),
        [(Op::Error, 6, vec![ErrorCode::BadRequest as u8])]
    );
}

#[test]
fn reads_and_changes_the_settings() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let a = request(&mut s, &mut d, Op::GetSettings, 1, &[]);
    let text = String::from_utf8(a[0].2.clone()).unwrap();
    assert_eq!(a[0].0, Op::Settings);
    assert!(
        text.contains("sound=on\n") && text.contains("backlight=50\n"),
        "{text}"
    );

    let events = write(
        &mut s,
        &mut d,
        &frame(Op::SetSettings, 2, b"sound=off\nbacklight=80\n"),
    );
    assert_eq!(answers(&s.take_output()), [(Op::Ok, 2, vec![])]);
    assert_eq!(events, [Event::SettingsChanged]);
    assert!(!d.settings.sound);
    assert_eq!(d.settings.backlight, 80);
}

#[test]
fn a_refused_setting_names_its_key_and_changes_nothing() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let a = request(
        &mut s,
        &mut d,
        Op::SetSettings,
        3,
        b"sound=off\nscroll_s=99\n",
    );
    let mut expected = vec![ErrorCode::BadSetting as u8];
    expected.extend_from_slice(b"scroll_s");
    assert_eq!(a, [(Op::Error, 3, expected)]);
    assert_eq!(d.settings, settings::Settings::default());
}

#[test]
fn receives_a_route() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let file = route_file();
    assert_eq!(
        send(&mut s, &mut d, Kind::Route, &file, None),
        Ok(file.len()),
        "each byte sent once"
    );
    assert_eq!(d.files, [(Kind::Route, file)]);
    assert_eq!(s.receiving(), None);
}

#[test]
fn resumes_a_route_after_the_link_dropped() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let file = route_file();
    assert!(file.len() > 2000, "{}", file.len());
    assert!(send(&mut s, &mut d, Kind::Route, &file, Some(8)).is_err());
    let (_, held, _) = s.receiving().unwrap();
    assert!(held > 0);
    let sent = send(&mut s, &mut d, Kind::Route, &file, None).unwrap();
    assert_eq!(sent, file.len() - held as usize, "only the missing part");
    assert_eq!(d.files, [(Kind::Route, file)]);
}

#[test]
fn reports_the_progress() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let file = route_file();
    let begin = Begin {
        kind: Kind::Route,
        size: file.len() as u32,
        crc: crc16_ccitt(&file),
    };
    let events = write(
        &mut s,
        &mut d,
        &frame(Op::TransferBegin, 1, &begin.to_bytes()),
    );
    assert_eq!(
        events,
        [Event::Receiving {
            kind: Kind::Route,
            received: 0,
            size: file.len() as u32
        }]
    );
}

#[test]
fn a_file_that_is_not_a_route_is_refused() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let junk = vec![0x55; 3000];
    assert_eq!(
        send(&mut s, &mut d, Kind::Route, &junk, None),
        Err(ErrorCode::BadRequest)
    );
    assert!(d.files.is_empty());
}

#[test]
fn the_device_decides_what_it_takes() {
    let (mut s, mut d) = (Session::new(), FakeDevice::default());
    let file = route_file();
    assert_eq!(
        send(&mut s, &mut d, Kind::Map, &file, None),
        Err(ErrorCode::BadRequest)
    );
    d.map = Some(Ok(100));
    assert_eq!(
        send(&mut s, &mut d, Kind::Map, &file, None),
        Err(ErrorCode::NoSpace)
    );
}
