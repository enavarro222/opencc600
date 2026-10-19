//! Sentence parsing into a [`Fix`], and `$PAIR` commands.
//!
//! No floating point: coordinates are degrees × 10⁷ (1 cm), computed from the minutes the receiver sends with
//! 5 decimals; other values are scaled integers.

use crate::checksum;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

/// Latest known state of the receiver.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Fix {
    /// RMC time and date (UTC).
    pub time: Option<Time>,
    pub date: Option<Date>,
    /// RMC status `A`: the position below is valid.
    pub valid: bool,
    /// Degrees × 10⁷, north / east positive.
    pub lat_e7: Option<i32>,
    pub lon_e7: Option<i32>,
    /// Ground speed, mm/s.
    pub speed_mm_s: Option<u32>,
    /// Course over ground, hundredths of a degree.
    pub course_cdeg: Option<u16>,
    /// GGA quality: 0 none, 1 GNSS, 2 DGNSS...
    pub quality: u8,
    pub sats_used: u8,
    /// Horizontal dilution of precision × 100.
    pub hdop_c: Option<u16>,
    /// Altitude above mean sea level, decimetres (GNSS altitude; the ride altitude comes from the barometer).
    pub alt_dm: Option<i32>,
    /// Satellites in view, all constellations (GSV).
    pub sats_in_view: u8,
    /// GGA time, to pair GGA and RMC of the same epoch.
    gga_time: Option<Time>,
    /// Satellites in view per GSV talker (GP, GL, GA, GB, GQ, GI...).
    in_view: [(u16, u8); 8],
}

/// Result of [`Fix::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Epoch {
    /// The sentence updated part of the fix.
    Partial,
    /// RMC arrived after the GGA of the same second: the fix is complete for this epoch.
    Complete,
    /// Not a sentence used for the fix.
    Ignored,
}

impl Fix {
    /// Updates the fix from one checked sentence body (`GNRMC,...`, as given by [`crate::LineReader`]).
    pub fn update(&mut self, body: &str) -> Epoch {
        let mut f = body.split(',');
        let Some(head) = f.next() else {
            return Epoch::Ignored;
        };
        if head.len() != 5 {
            return Epoch::Ignored;
        }
        let (talker, kind) = head.split_at(2);
        match kind {
            "RMC" => self.rmc(f),
            "GGA" => self.gga(f),
            "GSV" => self.gsv(talker, f),
            _ => Epoch::Ignored,
        }
    }

    fn rmc<'a>(&mut self, mut f: impl Iterator<Item = &'a str>) -> Epoch {
        // 1 time, 2 status, 3 lat, 4 N/S, 5 lon, 6 E/W, 7 speed kn, 8 course, 9 date ddmmyy
        let time = f.next().and_then(time);
        self.time = time;
        self.valid = f.next() == Some("A");
        let lat = coord(f.next(), f.next(), 2);
        let lon = coord(f.next(), f.next(), 3);
        let speed = f
            .next()
            .and_then(|s| fixed(s, 3))
            .map(|kn_milli| (kn_milli as u64 * 1852 / 3600) as u32);
        let course = f.next().and_then(|s| fixed(s, 2)).map(|c| c as u16);
        self.date = f.next().and_then(date);
        if self.valid {
            (self.lat_e7, self.lon_e7, self.speed_mm_s, self.course_cdeg) =
                (lat, lon, speed, course);
        } else {
            (self.lat_e7, self.lon_e7, self.speed_mm_s, self.course_cdeg) =
                (None, None, None, None);
        }
        if time.is_some() && time == self.gga_time {
            Epoch::Complete
        } else {
            Epoch::Partial
        }
    }

    fn gga<'a>(&mut self, mut f: impl Iterator<Item = &'a str>) -> Epoch {
        // 1 time, 2-5 position (taken from RMC), 6 quality, 7 sats used, 8 hdop, 9 altitude, 10 M
        self.gga_time = f.next().and_then(time);
        let mut f = f.skip(4);
        self.quality = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        self.sats_used = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        self.hdop_c = f
            .next()
            .and_then(|s| fixed(s, 2))
            .map(|h| h.min(u16::MAX as u32) as u16);
        self.alt_dm = f.next().and_then(|s| signed_fixed(s, 1));
        Epoch::Partial
    }

    fn gsv<'a>(&mut self, talker: &str, mut f: impl Iterator<Item = &'a str>) -> Epoch {
        // 1 messages, 2 message number, 3 satellites in view (this talker)
        let (Some(_), Some(_), Some(n)) = (
            f.next(),
            f.next(),
            f.next().and_then(|s| s.parse::<u8>().ok()),
        ) else {
            return Epoch::Ignored;
        };
        let key = u16::from_be_bytes([talker.as_bytes()[0], talker.as_bytes()[1]]);
        let slot = self
            .in_view
            .iter()
            .position(|&(k, _)| k == key)
            .or_else(|| self.in_view.iter().position(|&(k, _)| k == 0));
        if let Some(i) = slot {
            self.in_view[i] = (key, n);
        }
        self.sats_in_view = self
            .in_view
            .iter()
            .map(|&(_, n)| n)
            .fold(0u8, u8::saturating_add);
        Epoch::Partial
    }
}

/// `hhmmss.sss`
fn time(s: &str) -> Option<Time> {
    let b = s.as_bytes();
    if b.len() < 6 {
        return None;
    }
    let millis = s
        .get(6..)
        .filter(|f| !f.is_empty())
        .and_then(|f| fixed(f, 3))
        .unwrap_or(0) as u16;
    Some(Time {
        hour: two(b, 0)?,
        minute: two(b, 2)?,
        second: two(b, 4)?,
        millis,
    })
}

/// `ddmmyy`
fn date(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    if b.len() != 6 {
        return None;
    }
    Some(Date {
        day: two(b, 0)?,
        month: two(b, 2)?,
        year: 2000 + two(b, 4)? as u16,
    })
}

fn two(b: &[u8], i: usize) -> Option<u8> {
    let (h, l) = (
        b.get(i)?.checked_sub(b'0')?,
        b.get(i + 1)?.checked_sub(b'0')?,
    );
    (h < 10 && l < 10).then_some(h * 10 + l)
}

/// Unsigned decimal `int.frac` as an integer scaled by 10^`decimals` (extra digits truncated).
fn fixed(s: &str, decimals: u32) -> Option<u32> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let mut v: u32 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let digits = frac.as_bytes();
    for k in 0..decimals as usize {
        let d = match digits.get(k) {
            Some(&c) if c.is_ascii_digit() => (c - b'0') as u32,
            Some(_) => return None,
            None => 0,
        };
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    Some(v)
}

fn signed_fixed(s: &str, decimals: u32) -> Option<i32> {
    match s.strip_prefix('-') {
        Some(rest) => fixed(rest, decimals).map(|v| -(v as i32)),
        None => fixed(s, decimals).map(|v| v as i32),
    }
}

/// `dddmm.mmmmm` + hemisphere → degrees × 10⁷; `deg_digits` = 2 for latitude, 3 for longitude.
fn coord(value: Option<&str>, hemi: Option<&str>, deg_digits: usize) -> Option<i32> {
    let value = value?;
    let deg: u32 = value.get(..deg_digits)?.parse().ok()?;
    let min_e5 = fixed(value.get(deg_digits..)?, 5)?;
    let e7 = (deg * 10_000_000 + min_e5 * 100 / 60) as i32;
    match hemi? {
        "N" | "E" => Some(e7),
        "S" | "W" => Some(-e7),
        _ => None,
    }
}

/// Airoha acknowledge `PAIR001,<command>,<result>` → (command, result); result 0 = success.
pub fn pair_ack(body: &str) -> Option<(u16, u8)> {
    let mut f = body.strip_prefix("PAIR001,")?.split(',');
    Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?))
}

/// Writes `$<body>*hh\r\n` into `out`, returns its length (None if `out` is too small).
pub fn write_command(body: &str, out: &mut [u8]) -> Option<usize> {
    let n = body.len() + 6;
    let out = out.get_mut(..n)?;
    out[0] = b'$';
    out[1..=body.len()].copy_from_slice(body.as_bytes());
    let cs = checksum(body.as_bytes());
    let hex = b"0123456789ABCDEF";
    out[body.len() + 1..].copy_from_slice(&[
        b'*',
        hex[(cs >> 4) as usize],
        hex[(cs & 15) as usize],
        b'\r',
        b'\n',
    ]);
    Some(n)
}
