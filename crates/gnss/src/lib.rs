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
pub use nmea::{
    Aiding, Date, Epoch, Fix, OUTPUT_SETUP, Start, Time, aiding_request, is_started, is_turned_off, pair_ack,
    write_command,
};

/// Watches the receiver's output: silent for [`Silence::AFTER_MS`], it is powered on (through the nRF), again every
/// [`Silence::RETRY_MS`] while it stays silent. The nRF powers the receiver and can leave it off: on 2026-10-20 a
/// single power-on at the start of a silence left it off for 6 minutes of a ride. Only when silent: powering on a
/// running receiver restarts it (and it loses its output setup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Silence {
    last_data_ms: u64,
    /// When the last power-on was asked, during the current silence.
    powered_ms: Option<u64>,
    /// Silences since boot.
    pub count: u16,
}

impl Silence {
    pub const AFTER_MS: u64 = 3_000;
    pub const RETRY_MS: u64 = 10_000;

    pub const fn new(now_ms: u64) -> Self {
        Self { last_data_ms: now_ms, powered_ms: None, count: 0 }
    }

    /// Called at each poll with the bytes received since the last one; `true`: power the receiver on now.
    pub fn poll(&mut self, bytes: usize, now_ms: u64) -> bool {
        if bytes > 0 {
            self.last_data_ms = now_ms;
            self.powered_ms = None;
            return false;
        }
        if now_ms.saturating_sub(self.last_data_ms) < Self::AFTER_MS {
            return false;
        }
        match self.powered_ms {
            Some(at) if now_ms.saturating_sub(at) < Self::RETRY_MS => false,
            at => {
                if at.is_none() {
                    self.count += 1;
                }
                self.powered_ms = Some(now_ms);
                true
            }
        }
    }

    /// Milliseconds since the last data.
    pub fn silent_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_data_ms)
    }
}

#[cfg(test)]
mod silence_tests {
    use super::Silence;

    /// Polls every 100 ms from `from` to `to` (ms) with no data; the times a power-on is asked.
    fn quiet(s: &mut Silence, from: u64, to: u64) -> ([u64; 16], usize) {
        let mut at = [0; 16];
        let mut n = 0;
        for t in (from..to).step_by(100) {
            if s.poll(0, t) {
                at[n] = t;
                n += 1;
            }
        }
        (at, n)
    }

    #[test]
    fn a_receiver_left_off_is_powered_on_again_until_it_talks() {
        let mut s = Silence::new(0);
        assert!(!s.poll(879, 2_000));
        // silent for the 6 minutes of the 2026-10-20 ride: at 3 s, then every 10 s
        let (at, n) = quiet(&mut s, 2_100, 62_100);
        assert_eq!(&at[..n], &[5_000, 15_000, 25_000, 35_000, 45_000, 55_000]);
        assert_eq!(s.count, 1, "one silence");
        // it talks again: nothing more, and the next silence counts
        assert!(!s.poll(500, 62_100));
        let (_, n) = quiet(&mut s, 62_200, 65_000);
        assert_eq!(n, 0, "less than 3 s");
        assert!(s.poll(0, 65_100));
        assert_eq!(s.count, 2);
    }

    #[test]
    fn a_talking_receiver_is_left_alone() {
        let mut s = Silence::new(0);
        // bursts once a second (the NMEA epoch), polls in between with nothing
        for t in (0..60_000).step_by(100) {
            assert!(!s.poll(if t % 1_000 == 0 { 1_500 } else { 0 }, t));
        }
        assert_eq!(s.count, 0);
    }
}
