//! The two remotes captured on 2026-11-03 (tools/ble_remote_probe.py): their report maps, then every press their
//! buttons made, back to the buttons.

use remote::hid::{CONSUMER, Layout};
use remote::{Buttons, Learned, Learner, Paired, Presses, Signature};

const VOLUME_UP: u32 = (CONSUMER as u32) << 16 | 0xE9;
const VOLUME_DOWN: u32 = (CONSUMER as u32) << 16 | 0xEA;

struct Capture {
    map: Vec<u8>,
    reports: Vec<(u64, u8, Vec<u8>)>,
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn capture(text: &str) -> Capture {
    let mut c = Capture { map: Vec::new(), reports: Vec::new() };
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["map", m] => c.map = hex(m),
            [t, id, data] => c.reports.push((t.parse().unwrap(), id.parse().unwrap(), hex(data))),
            _ => panic!("{line}"),
        }
    }
    c
}

/// Every press, in order, with its time.
fn presses(c: &Capture) -> Vec<(u64, Signature)> {
    let mut p = Presses::new(Layout::parse(&c.map));
    let mut out = Vec::new();
    for (t, id, data) in &c.reports {
        if let Some(s) = p.poll(*t) {
            out.push((*t, s));
        }
        p.report(*id, data, *t);
    }
    if let Some(s) = p.poll(u64::MAX) {
        out.push((u64::MAX, s));
    }
    out
}

/// The different buttons among the presses.
fn buttons(presses: &[(u64, Signature)]) -> Vec<Signature> {
    let mut out: Vec<Signature> = Vec::new();
    for (_, s) in presses {
        if !out.iter().any(|b| b.matches(s)) {
            out.push(*s);
        }
    }
    out
}

fn stroke(from: (u16, u16), to: (u16, u16)) -> Signature {
    Signature::Stroke { from, to }
}

fn m3() -> Capture {
    capture(include_str!("fixtures/ble-m3.txt"))
}

fn j6() -> Capture {
    capture(include_str!("fixtures/yiser-j6.txt"))
}

#[test]
fn the_report_maps() {
    let m3 = Layout::parse(&m3().map);
    let t = m3.touch.unwrap();
    assert_eq!((t.tip.report, t.tip.bit, t.x.bit, t.x.size, t.y.bit, t.x_max, t.y_max), (6, 0, 8, 16, 24, 4095, 4095));
    // the consumer bitmap (report 3) and array (report 2)
    assert_eq!(m3.keys::<4>(3, &[0x01, 0x00]), Some(([VOLUME_UP, 0, 0, 0], 1)));
    assert_eq!(m3.keys::<4>(3, &[0x02, 0x00]), Some(([VOLUME_DOWN, 0, 0, 0], 1)));
    assert_eq!(m3.keys::<4>(2, &[0xE9, 0x00]), Some(([VOLUME_UP, 0, 0, 0], 1)));
    assert_eq!(m3.keys::<4>(2, &[0x00, 0x00]).map(|k| k.1), Some(0));
    assert_eq!(m3.keys::<4>(6, &[0x07, 0, 0, 0, 0]), None);

    let j6 = Layout::parse(&j6().map);
    let t = j6.touch.unwrap();
    assert_eq!((t.tip.report, t.x.bit, t.y.bit, t.x_max, t.y_max), (1, 8, 24, 1000, 1000));
    // 24 consumer keys in report 3: volume up is the 16th
    assert_eq!(j6.keys::<4>(3, &[0x00, 0x80, 0x00]), Some(([VOLUME_UP, 0, 0, 0], 1)));
    assert_eq!(j6.keys::<4>(3, &[0x00, 0x00, 0x01]), Some(([VOLUME_DOWN, 0, 0, 0], 1)));
}

#[test]
fn the_ble_m3_presses() {
    let p = presses(&m3());
    // 6 buttons: four swipes, a tap in the middle, the photo button (volume down, then up, in turn, with a tap)
    let b = buttons(&p);
    assert_eq!(
        b,
        [
            stroke((316, 570), (23, 570)),
            stroke((247, 403), (247, 403)),
            stroke((247, 623), (247, 183)),
            stroke((121, 570), (511, 570)),
            stroke((247, 183), (247, 623)),
            Signature::Key(VOLUME_DOWN),
            Signature::Key(VOLUME_UP),
        ]
    );
    // the photo button is one press: its key and its tap together
    assert_eq!(p.len(), 32);
    // the photo button alternates
    let keys: Vec<u32> = p.iter().filter_map(|(_, s)| if let Signature::Key(k) = s { Some(*k) } else { None }).collect();
    assert!(keys.windows(2).all(|w| w[0] != w[1]), "{keys:x?}");
}

#[test]
fn the_yiser_j6_presses() {
    let p = presses(&j6());
    let b = buttons(&p);
    assert_eq!(
        b,
        [
            stroke((400, 390), (50, 390)),
            stroke((300, 200), (290, 900)),
            stroke((300, 800), (290, 100)),
            stroke((300, 500), (300, 500)),
            stroke((150, 390), (450, 390)),
            Signature::Key(VOLUME_UP),
            stroke((420, 850), (420, 850)),
            Signature::Key(VOLUME_DOWN),
        ]
    );
    assert_eq!(p.len(), 18);
}

#[test]
fn learning_the_buttons_of_a_ble_m3() {
    let p = presses(&m3());
    let sig = |i: usize| p[i].1;
    let photo = p.iter().position(|(_, s)| matches!(s, Signature::Key(_))).unwrap();
    let mut l = Learner::new(Buttons::default());
    assert_eq!(l.key(), Some(2));
    // the main key: the photo button, twice (two different keys)
    assert_eq!(l.press(sig(photo)), Learned::Again);
    assert_eq!(l.presses(), 1);
    assert_eq!(l.press(sig(photo + 1)), Learned::Done(2));
    // the page key: the swipe up; the photo button again is refused
    assert_eq!(l.key(), Some(1));
    assert_eq!(l.press(sig(photo)), Learned::Taken(2));
    assert_eq!(l.press(sig(2)), Learned::Again);
    assert_eq!(l.press(sig(2)), Learned::Done(1));
    // back: skipped
    l.skip();
    assert_eq!(l.key(), None);
    assert_eq!(l.press(sig(0)), Learned::Finished);
    let buttons = l.buttons();
    assert_eq!(buttons.key(&Signature::Key(VOLUME_UP)), Some(2));
    assert_eq!(buttons.key(&Signature::Key(VOLUME_DOWN)), Some(2));
    assert_eq!(buttons.key(&sig(2)), Some(1));
    assert_eq!(buttons.key(&sig(0)), None);
    assert!(!buttons.has(0));
}

#[test]
fn a_skipped_key_keeps_its_old_button() {
    let tap = stroke((300, 500), (300, 500));
    let left = stroke((400, 390), (50, 390));
    let mut old = Buttons::default();
    old.add(tap, 2);
    old.add(left, 0);
    let mut l = Learner::new(old);
    // the old buttons hold until their key is learned again
    assert_eq!(l.buttons(), old);
    l.skip();
    // the page key: the left swipe, the old back button, is free in this round
    assert_eq!(l.press(left), Learned::Again);
    assert_eq!(l.press(left), Learned::Done(1));
    l.skip();
    let b = l.buttons();
    assert_eq!(b.key(&tap), Some(2));
    // the left swipe moved to the page key
    assert_eq!(b.key(&left), Some(1));
    assert!(!b.has(0));
}

#[test]
fn the_remote_file() {
    let mut p = Paired {
        address: remote::Address { addr: [0xC6, 0x11, 0x2A, 0x98, 0x07, 0x2A], random: false },
        name: remote::Name::new(b"BLE-M3"),
        bond: Some(remote::Bond {
            ltk: 0x0123_4567_89AB_CDEF_0011_2233_4455_6677,
            ediv: 0xBEEF,
            rand: [1, 2, 3, 4, 5, 6, 7, 8],
            key_len: 16,
            level: 1,
            identity: remote::Address { addr: [0xC6, 0x11, 0x2A, 0x98, 0x07, 0x2A], random: false },
            irk: None,
        }),
        buttons: Buttons::default(),
    };
    p.buttons.add(Signature::Key(VOLUME_UP), 2);
    p.buttons.add(stroke((247, 623), (247, 183)), 1);
    let mut text = String::new();
    p.write(&mut text).unwrap();
    assert_eq!(
        text,
        "address 2A:07:98:2A:11:C6 public\n\
         name BLE-M3\n\
         bond 0123456789abcdef0011223344556677 beef 0102030405060708 16 1 2A:07:98:2A:11:C6 public -\n\
         button 2 key 000c00e9\n\
         button 1 stroke 247 623 247 183\n"
    );
    assert_eq!(Paired::parse(&text), Some(p));
    // with an IRK, without a bond, an unknown line
    p.bond.as_mut().unwrap().irk = Some(42);
    let mut text = String::new();
    p.write(&mut text).unwrap();
    assert_eq!(Paired::parse(&text), Some(p));
    assert_eq!(Paired::parse("address 2A:07:98:2A:11:C6 random\nfoo bar\n").map(|p| p.bond), Some(None));
    assert_eq!(Paired::parse("name x\n"), None);
}
