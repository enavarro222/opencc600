//! NMEA 0183 for the CC600 GNSS receiver (Airoha AG3352Q), hardware independent.
//!
//! [`LineReader`] cuts the byte stream into sentences and checks their checksum.

#![no_std]

/// Longest sentence kept; NMEA allows 82 characters, Airoha `$PAIR` replies stay below.
pub const MAX_LINE: usize = 128;

/// XOR of the bytes between `$` and `*`.
pub fn checksum(body: &[u8]) -> u8 {
    body.iter().fold(0, |x, b| x ^ b)
}

/// Counters, for the link health on the screen and in the logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub bytes: u32,
    pub sentences: u32,
    pub bad_checksum: u32,
    /// A `$` before the end of the previous line: that line was cut (bytes lost).
    pub cut: u32,
    /// Longer than [`MAX_LINE`], dropped: the Airoha `$PAIRSAT` satellite reports (~300 chars, not needed).
    pub too_long: u32,
    /// No `*hh` checksum, or not UTF-8.
    pub malformed: u32,
}

/// Cuts a byte stream into checked NMEA sentences.
pub struct LineReader {
    buf: [u8; MAX_LINE],
    len: usize,
    in_line: bool,
    pub stats: Stats,
}

impl Default for LineReader {
    fn default() -> Self {
        Self::new()
    }
}

impl LineReader {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_LINE],
            len: 0,
            in_line: false,
            stats: Stats {
                bytes: 0,
                sentences: 0,
                bad_checksum: 0,
                cut: 0,
                too_long: 0,
                malformed: 0,
            },
        }
    }

    /// Feeds received bytes; `f` gets every sentence with a valid checksum, without `$`, `*hh` and CR LF
    /// (e.g. `GNRMC,123519.00,A,...`).
    pub fn feed(&mut self, data: &[u8], mut f: impl FnMut(&str)) {
        for &b in data {
            self.stats.bytes = self.stats.bytes.wrapping_add(1);
            match b {
                b'$' => {
                    if self.in_line {
                        self.stats.cut += 1;
                    }
                    self.in_line = true;
                    self.len = 0;
                }
                b'\r' | b'\n' => {
                    if self.in_line {
                        self.in_line = false;
                        if let Some(body) = check(&self.buf[..self.len], &mut self.stats) {
                            self.stats.sentences += 1;
                            f(body);
                        }
                    }
                }
                _ if self.in_line => {
                    if self.len == MAX_LINE {
                        self.in_line = false;
                        self.stats.too_long += 1;
                    } else {
                        self.buf[self.len] = b;
                        self.len += 1;
                    }
                }
                _ => {}
            }
        }
    }
}

/// The sentence body if `line` (without `$`) ends with a valid `*hh` checksum.
fn check<'a>(line: &'a [u8], stats: &mut Stats) -> Option<&'a str> {
    let star = match line.len().checked_sub(3) {
        Some(i) if line[i] == b'*' => i,
        _ => {
            stats.malformed += 1;
            return None;
        }
    };
    let want = core::str::from_utf8(&line[star + 1..])
        .ok()
        .and_then(|h| u8::from_str_radix(h, 16).ok());
    if want != Some(checksum(&line[..star])) {
        stats.bad_checksum += 1;
        return None;
    }
    match core::str::from_utf8(&line[..star]) {
        Ok(body) => Some(body),
        Err(_) => {
            stats.malformed += 1;
            None
        }
    }
}

mod nmea;
pub use nmea::{Date, Epoch, Fix, OUTPUT_SETUP, Time, is_turned_off, pair_ack, write_command};
