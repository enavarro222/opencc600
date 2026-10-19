//! Fixtures from c606-oss `docs/HARDWARE.md`, logged on a real C606 (PoC, 2026-09-17).
//! The PoC log stops before the CRC, so the frames are completed with [`with_crc`], except the vendor's
//! precomputed LCD reset frame, which carries its own.

use nrf_link::{Decoder, Frame, KeyAction, Message, RtcTime, command, crc16, encode};
use proptest::prelude::*;

fn with_crc(bytes: &[u8]) -> Vec<u8> {
    let mut f = bytes.to_vec();
    f.extend_from_slice(&crc16(bytes).to_le_bytes());
    f
}

fn frames(data: &[u8]) -> Vec<Frame> {
    let mut out = Vec::new();
    Decoder::new().feed(data, |f| out.push(f));
    out
}

fn parse_one(header_and_payload: &[u8]) -> Message {
    let got = frames(&with_crc(header_and_payload));
    assert_eq!(got.len(), 1, "{header_and_payload:02X?}");
    Message::parse(&got[0])
}

#[test]
fn crc_is_xmodem() {
    assert_eq!(crc16(b"123456789"), 0x31C3);
}

#[test]
fn vendor_precomputed_frame() {
    // `mg_panel_axs1523_reset` (C706): LCD reset pulse, frame stored in the vendor firmware with its CRC.
    let vendor = [
        0xA5, 0x0C, 0x6F, 0xF1, 0x02, 0x10, 0xE2, 0x02, 0x09, 0x00, 0x00, 0x02, 0x00, 0x00, 0x7A,
        0x3C,
    ];
    let mut out = [0; 16];
    let n = encode(2, 0x10, &vendor[6..14], &mut out).unwrap();
    assert_eq!(&out[..n], &vendor);
    assert_eq!(frames(&vendor).len(), 1);
}

#[test]
fn commands() {
    let on = command::power_on();
    assert_eq!(
        &on[..14],
        &[
            0xA5, 0x0C, 0x6F, 0xF1, 0x02, 0x10, 0xE2, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00
        ]
    );
    assert_eq!(command::check_power_on_reason()[4], 1);
    assert_eq!(command::power_off()[11], 0);
    let jingle = command::misc(command::SOUND_JINGLE);
    assert_eq!(
        &jingle[6..14],
        &[0xE2, 0x01, 0x17, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
    for f in [
        on,
        command::check_power_on_reason(),
        command::power_off(),
        command::gps_power(1),
    ] {
        assert_eq!(frames(&f).len(), 1);
    }
}

#[test]
fn battery() {
    let m = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x00, 0x52, 0xFF, 0xFF, 0xFF, 0xF7, 0x10, 0x64, 0xFF,
    ]);
    assert_eq!(
        m,
        Message::Battery {
            millivolts: 4343,
            percent: 100
        }
    );
}

#[test]
fn status() {
    let m = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xF0, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(m, Message::Status(0));
}

#[test]
fn power_on_replies() {
    let ack = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x05, 0x10, 0xE2, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(ack, Message::Ack);
    let reason = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xE2, 0x02, 0x00, 0x00, 0xFF, 0x04, 0x00, 0xFF,
    ]);
    assert_eq!(reason, Message::PowerOnReason(4));
}

#[test]
fn version() {
    let m = parse_one(&[
        0xA5, 0x0E, 0x6F, 0xF1, 0x04, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
        0x13,
    ]);
    assert_eq!(
        m,
        Message::Version {
            major: 0,
            minor: 2,
            patch: 19
        }
    );
}

#[test]
fn rtc() {
    // `.. 3A 0C 91 08 7E` = 2026-09-17 12:58 UTC (day byte 0x91 & 0x1F = 17).
    let m = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x00, 0x53, 0xFF, 0x05, 0x3A, 0x0C, 0x91, 0x08, 0x7E,
    ]);
    let want = RtcTime {
        year: 2026,
        month: 9,
        day: 17,
        hour: 12,
        minute: 58,
        second: 5,
    };
    assert_eq!(m, Message::Rtc(want));
}

#[test]
fn keys() {
    let key = |idx, evt| {
        parse_one(&[
            0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0x49, idx, 0, 0, 0, 0, evt, 0x80,
        ])
    };
    assert_eq!(
        key(0, 1),
        Message::Key {
            key: 0,
            action: KeyAction::Click
        }
    );
    assert_eq!(
        key(1, 4),
        Message::Key {
            key: 1,
            action: KeyAction::LongPress
        }
    );
    assert_eq!(
        key(2, 5),
        Message::Key {
            key: 2,
            action: KeyAction::LongRelease
        }
    );
    assert_eq!(
        key(2, 9),
        Message::Key {
            key: 2,
            action: KeyAction::Other(9)
        }
    );
}

#[test]
fn baro() {
    // `AD 0B` = 29.89 °C, `BC 2B 92 00` = 957.9 hPa.
    let m = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xF1, 0x03, 0xAD, 0x0B, 0xBC, 0x2B, 0x92, 0x00,
    ]);
    assert_eq!(
        m,
        Message::Baro {
            centi_celsius: 2989,
            centi_pascal: 9_579_452
        }
    );
}

#[test]
fn cc600_stream_subtypes() {
    // CC600 (nRF fw 1.2.16), 3.3 Hz each: 04 = ambient light, 0A = unknown.
    let m = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xF1, 0x04, 0x58, 0x02, 0x00, 0x00, 0x28, 0x04,
    ]);
    assert_eq!(
        m,
        Message::Light {
            level: 0x0428,
            param: 600
        }
    );
    let m = parse_one(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xF1, 0x0A, 0xB4, 0x0B, 0x00, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(
        m,
        Message::Stream {
            sub: 0x0A,
            data: [0xB4, 0x0B, 0, 0, 0, 0]
        }
    );
}

#[test]
fn cc600_frames_with_their_crc() {
    // Received as is on the CC600, CRC included.
    let f = [
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xF1, 0x04, 0x58, 0x02, 0x00, 0x00, 0x28, 0x04, 0x3D,
        0xCB,
    ];
    assert_eq!(frames(&f).len(), 1);
    let f = [
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x10, 0xF1, 0x0A, 0xB4, 0x0B, 0x00, 0x00, 0x00, 0x00, 0xD9,
        0x1E,
    ];
    assert_eq!(frames(&f).len(), 1);
}

#[test]
fn short_payload_is_unknown() {
    assert_eq!(
        parse_one(&[0xA5, 0x06, 0x6F, 0xF1, 0x04, 0x00, 0x52, 0xFF]),
        Message::Unknown
    );
}

#[test]
fn resync_after_garbage_and_bad_crc() {
    let good = with_crc(&[
        0xA5, 0x0C, 0x6F, 0xF1, 0x04, 0x00, 0x52, 0xFF, 0xFF, 0xFF, 0xF7, 0x10, 0x64, 0xFF,
    ]);
    let mut bad = good.clone();
    bad[10] ^= 0xFF;
    let mut stream = vec![0x00, 0xA5, 0xA5, 0x13];
    stream.extend(&bad);
    stream.extend(&good);
    stream.extend(&good);
    let mut dec = Decoder::new();
    let mut n = 0;
    dec.feed(&stream, |f| {
        assert_eq!(f.bytes(), &good[..]);
        n += 1;
    });
    assert_eq!(n, 2);
    assert_eq!(dec.crc_errors, 1);
}

proptest! {
    /// Any payload survives encode → garbage → split delivery → decode.
    #[test]
    fn roundtrip(
        kind in 1u8..=5,
        cmd: u8,
        payload in prop::collection::vec(any::<u8>(), 0..=nrf_link::MAX_PAYLOAD),
        garbage in prop::collection::vec(any::<u8>(), 0..64),
        split in any::<prop::sample::Index>(),
    ) {
        let mut frame = [0; nrf_link::MAX_FRAME];
        let n = encode(kind, cmd, &payload, &mut frame).unwrap();
        // No sync byte in the garbage: a fake header (A5 + long N) would hold the real frame back until enough
        // bytes arrive to fail its CRC, i.e. after the end of this finite stream (covered by the resync test).
        let mut stream: Vec<u8> = garbage.iter().map(|&b| if b == 0xA5 { 0 } else { b }).collect();
        stream.extend_from_slice(&frame[..n]);
        let at = split.index(stream.len() + 1);
        let mut dec = Decoder::new();
        let mut got = Vec::new();
        dec.feed(&stream[..at], |f| got.push(f));
        dec.feed(&stream[at..], |f| got.push(f));
        prop_assert_eq!(got.len(), 1);
        prop_assert_eq!(got[0].kind(), kind);
        prop_assert_eq!(got[0].cmd(), cmd);
        prop_assert_eq!(got[0].payload(), &payload[..]);
    }

    /// Random bytes never panic the decoder.
    #[test]
    fn no_panic_on_noise(data in prop::collection::vec(any::<u8>(), 0..2048)) {
        let mut dec = Decoder::new();
        dec.feed(&data, |f| { let _ = Message::parse(&f); });
    }
}
