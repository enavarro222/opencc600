//! A BLE remote (HID over GATT) as a second keypad of the CC600, hardware independent: its report map read
//! ([`hid`]), its reports back to button presses ([`press`]), the buttons learned for the device's keys
//! ([`Buttons`], [`Learner`]) and the `/remote` file that keeps the remote, its bond and its buttons ([`Paired`]).
//!
//! Each button is learned for a key, by pressing it twice: no table per remote model, and a button that alternates
//! two keys (the photo button of a BLE-M3 sends volume down, then up) is learned with both.

#![no_std]

pub mod hid;
pub mod press;

use core::fmt;

pub use press::{Presses, Signature};

/// The device's keys, numbered as the nRF's key frames: 0 top left, 1 bottom left, 2 bottom right.
pub const KEYS: u8 = 3;

/// The order the keys are learned in: the main key first, then the page key, then back.
pub const LEARN_ORDER: [u8; KEYS as usize] = [2, 1, 0];

/// Buttons learned at most (a key may have two: the alternating ones).
pub const BUTTONS: usize = 8;

/// The learned buttons: a signature, the key it presses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons {
    map: [Option<(Signature, u8)>; BUTTONS],
}

impl Buttons {
    /// The key this button presses.
    pub fn key(&self, s: &Signature) -> Option<u8> {
        self.map.iter().flatten().find(|(b, _)| b.matches(s)).map(|(_, k)| *k)
    }

    /// `s` now presses `key` (a button that pressed another key no longer does); `false` if full.
    pub fn add(&mut self, s: Signature, key: u8) -> bool {
        for e in self.map.iter_mut() {
            if e.is_some_and(|(b, _)| b.matches(&s)) {
                *e = None;
            }
        }
        match self.map.iter_mut().find(|e| e.is_none()) {
            Some(e) => {
                *e = Some((s, key));
                true
            }
            None => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Signature, u8)> {
        self.map.iter().flatten()
    }

    /// Some button presses this key.
    pub fn has(&self, key: u8) -> bool {
        self.iter().any(|(_, k)| *k == key)
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

/// What a press did while learning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Learned {
    /// The first press for the key: again, to be sure.
    Again,
    /// The key has its button; the next one is learned now (or none: [`Learner::key`]).
    Done(u8),
    /// This button already presses an earlier key of this learning.
    Taken(u8),
    /// Nothing left to learn.
    Finished,
}

/// Learning the buttons, key after key ([`LEARN_ORDER`]), each from two presses of its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Learner {
    step: u8,
    first: Option<Signature>,
    learned: Buttons,
    old: Buttons,
}

impl Learner {
    /// Starts over; a key skipped keeps its button of `old`.
    pub fn new(old: Buttons) -> Self {
        Learner { step: 0, first: None, learned: Buttons::default(), old }
    }

    /// The key being learned, `None` once all are.
    pub fn key(&self) -> Option<u8> {
        LEARN_ORDER.get(self.step as usize).copied()
    }

    /// Presses already had for the key being learned (0 or 1).
    pub fn presses(&self) -> u8 {
        self.first.is_some() as u8
    }

    pub fn press(&mut self, s: Signature) -> Learned {
        let Some(key) = self.key() else { return Learned::Finished };
        if let Some(k) = self.learned.key(&s) {
            return Learned::Taken(k);
        }
        let Some(first) = self.first.take() else {
            self.first = Some(s);
            return Learned::Again;
        };
        self.learned.add(first, key);
        if !s.matches(&first) {
            self.learned.add(s, key);
        }
        self.step += 1;
        Learned::Done(key)
    }

    /// The key being learned keeps its old button (or has none).
    pub fn skip(&mut self) {
        let Some(key) = self.key() else { return };
        for (s, _) in self.old.iter().filter(|(_, k)| *k == key) {
            if self.learned.key(s).is_none() {
                self.learned.add(*s, key);
            }
        }
        self.first = None;
        self.step += 1;
    }

    /// The buttons learned so far, the keys not reached yet keeping their old ones.
    pub fn buttons(&self) -> Buttons {
        let mut out = self.learned;
        for &key in LEARN_ORDER.iter().skip(self.step as usize) {
            for (s, _) in self.old.iter().filter(|(_, k)| *k == key) {
                if out.key(s).is_none() {
                    out.add(*s, key);
                }
            }
        }
        out
    }
}

/// A Bluetooth address, its bytes as on the air (least significant first).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Address {
    pub addr: [u8; 6],
    pub random: bool,
}

/// What a pairing with bonding left (trouble-host's `BondInformation`, in plain types): the keys to encrypt again
/// at the next connection without pairing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bond {
    pub ltk: u128,
    /// Legacy pairing's key identifiers (0 for LE Secure Connections).
    pub ediv: u16,
    pub rand: [u8; 8],
    pub key_len: u8,
    /// 0 none, 1 encrypted, 2 encrypted and authenticated.
    pub level: u8,
    pub identity: Address,
    pub irk: Option<u128>,
}

/// A name as the screens show it: printable ASCII, cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; 18],
    len: u8,
}

impl Name {
    pub fn new(raw: &[u8]) -> Self {
        let mut n = Name::default();
        for &b in raw {
            if n.len as usize == n.bytes.len() {
                break;
            }
            n.bytes[n.len as usize] = if (0x20..0x7F).contains(&b) { b } else { b'?' };
            n.len += 1;
        }
        while n.len > 0 && n.bytes[n.len as usize - 1] == b' ' {
            n.len -= 1;
        }
        n
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}

/// The paired remote: the `/remote` file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Paired {
    pub address: Address,
    pub name: Name,
    pub bond: Option<Bond>,
    pub buttons: Buttons,
}

impl Paired {
    /// The file's text; `None` without an address. Lines it does not know are skipped.
    pub fn parse(text: &str) -> Option<Paired> {
        let mut p = Paired::default();
        let mut has_address = false;
        for line in text.lines() {
            let (word, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
            match word {
                "address" => {
                    if let Some(a) = address(rest) {
                        p.address = a;
                        has_address = true;
                    }
                }
                "name" => p.name = Name::new(rest.as_bytes()),
                "bond" => p.bond = Bond::parse(rest),
                "button" => {
                    if let Some((s, k)) = button(rest) {
                        p.buttons.add(s, k);
                    }
                }
                _ => {}
            }
        }
        has_address.then_some(p)
    }

    pub fn write(&self, w: &mut impl fmt::Write) -> fmt::Result {
        writeln!(w, "address {}", Shown(&self.address))?;
        if !self.name.as_str().is_empty() {
            writeln!(w, "name {}", self.name.as_str())?;
        }
        if let Some(b) = &self.bond {
            write!(w, "bond {:032x} {:04x} ", b.ltk, b.ediv)?;
            for r in b.rand {
                write!(w, "{r:02x}")?;
            }
            write!(w, " {} {} {}", b.key_len, b.level, Shown(&b.identity))?;
            match b.irk {
                Some(irk) => writeln!(w, " {irk:032x}")?,
                None => writeln!(w, " -")?,
            }
        }
        for (s, k) in self.buttons.iter() {
            match s {
                Signature::Key(code) => writeln!(w, "button {k} key {code:08x}")?,
                Signature::Stroke { from, to } => {
                    writeln!(w, "button {k} stroke {} {} {} {}", from.0, from.1, to.0, to.1)?
                }
            }
        }
        Ok(())
    }
}

/// An address as people write it: most significant byte first, then `public` or `random`.
struct Shown<'a>(&'a Address);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let a = self.0.addr;
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} {}",
            a[5],
            a[4],
            a[3],
            a[2],
            a[1],
            a[0],
            if self.0.random { "random" } else { "public" }
        )
    }
}

fn address(text: &str) -> Option<Address> {
    let mut words = text.split_whitespace();
    address_of(words.next()?, words.next()?)
}

fn address_of(hex: &str, kind: &str) -> Option<Address> {
    let random = match kind {
        "random" => true,
        "public" => false,
        _ => return None,
    };
    let mut addr = [0; 6];
    let mut parts = hex.split(':');
    for i in (0..6).rev() {
        addr[i] = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    parts.next().is_none().then_some(Address { addr, random })
}

impl Bond {
    /// A `bond` line of the file, without its first word.
    pub fn parse(text: &str) -> Option<Bond> {
        bond(text)
    }
}

fn bond(text: &str) -> Option<Bond> {
    let w: [&str; 8] = {
        let mut out = [""; 8];
        let mut it = text.split_whitespace();
        for o in out.iter_mut() {
            *o = it.next()?;
        }
        out
    };
    let rand_hex = w[2];
    if rand_hex.len() != 16 {
        return None;
    }
    let mut rand = [0; 8];
    for (i, r) in rand.iter_mut().enumerate() {
        *r = u8::from_str_radix(rand_hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    let identity = address_of(w[5], w[6])?;
    Some(Bond {
        ltk: u128::from_str_radix(w[0], 16).ok()?,
        ediv: u16::from_str_radix(w[1], 16).ok()?,
        rand,
        key_len: w[3].parse().ok()?,
        level: w[4].parse().ok()?,
        identity,
        irk: match w[7] {
            "-" => None,
            hex => Some(u128::from_str_radix(hex, 16).ok()?),
        },
    })
}

fn button(text: &str) -> Option<(Signature, u8)> {
    let mut w = text.split_whitespace();
    let key: u8 = w.next()?.parse().ok().filter(|k| *k < KEYS)?;
    let s = match w.next()? {
        "key" => Signature::Key(u32::from_str_radix(w.next()?, 16).ok()?),
        "stroke" => {
            let mut n = || w.next()?.parse::<u16>().ok();
            Signature::Stroke { from: (n()?, n()?), to: (n()?, n()?) }
        }
        _ => return None,
    };
    Some((s, key))
}
