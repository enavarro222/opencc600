//! Device settings, hardware independent: a small text file of `key=value` lines (`/settings` on the storage), read
//! with defaults for what is missing or invalid, unknown keys ignored, so that an older or newer file still loads.
//! The firmware writes the whole file in one littlefs commit (the old one stays until the new one is complete).
//!
//! Time zone: a POSIX TZ rule, like `CET-1CEST,M3.5.0,M10.5.0/3` (central Europe: UTC+1, summer time UTC+2 from the
//! last Sunday of March 02:00 to the last Sunday of October 03:00), so that summer time follows by itself.

#![no_std]

use core::fmt::{self, Write};

/// What the settings file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Beeps on or off.
    pub sound: bool,
    pub tz: Tz,
    pub theme: Theme,
    /// Backlight, percent.
    pub backlight: u8,
    /// The app's one question: ride to explore or train (sets the ride pages).
    pub usage: Usage,
    /// Page auto-scroll, seconds a page.
    pub scroll_s: u8,
    /// The paired sensors: heart rate, speed and cadence, power (one device can be several).
    pub hr: Sensor,
    pub speed_cadence: Sensor,
    pub power: Sensor,
    /// The loaded route is followed; off: set aside for a free ride (it stays stored). A new route is followed.
    pub follow_route: bool,
    /// Switch off after this many minutes still during a ride (the ride kept); 0: never.
    pub auto_off_min: u8,
    /// The trip log (`/logs`, a line a second while switched on, ~5 times what a ride takes): for bring-up.
    pub trip_log: bool,
}

/// A paired sensor: `none`, `ble:r:E0:E2:58:63:35:07` (random address, `p:` public; most significant byte first,
/// as phones show it) or `ant:121:12345:1` (ANT+ device type, device number, transmission type).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sensor {
    #[default]
    None,
    Ble {
        random: bool,
        /// Least significant byte first, as on the air.
        addr: [u8; 6],
    },
    Ant {
        /// The ANT+ device type: 120 heart rate, 121 speed and cadence, 122 cadence, 123 speed, 11 power.
        device_type: u8,
        number: u16,
        transmission: u8,
    },
}

impl Sensor {
    pub fn parse(v: &str) -> Option<Self> {
        if v == "none" {
            return Some(Self::None);
        }
        if let Some(rest) = v.strip_prefix("ble:") {
            let (kind, hex) = rest.split_once(':')?;
            let random = match kind {
                "r" => true,
                "p" => false,
                _ => return None,
            };
            let mut addr = [0; 6];
            let mut bytes = hex.split(':');
            for b in addr.iter_mut().rev() {
                let h = bytes.next()?;
                if h.len() != 2 {
                    return None;
                }
                *b = u8::from_str_radix(h, 16).ok()?;
            }
            return bytes.next().is_none().then_some(Self::Ble { random, addr });
        }
        let (device_type, rest) = v.strip_prefix("ant:")?.split_once(':')?;
        let (number, transmission) = rest.split_once(':')?;
        Some(Self::Ant {
            device_type: device_type.parse().ok()?,
            number: number.parse().ok()?,
            transmission: transmission.parse().ok()?,
        })
    }
}

impl fmt::Display for Sensor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::None => f.write_str("none"),
            Self::Ble { random, addr } => {
                f.write_str(if random { "ble:r" } else { "ble:p" })?;
                for b in addr.iter().rev() {
                    write!(f, ":{b:02X}")?;
                }
                Ok(())
            }
            Self::Ant { device_type, number, transmission } => write!(f, "ant:{device_type}:{number}:{transmission}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Day,
    Night,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Explore,
    Train,
}

/// The default time zone: France.
pub const PARIS: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound: true,
            tz: Tz::parse(PARIS).unwrap(),
            theme: Theme::Day,
            backlight: 50,
            usage: Usage::Explore,
            scroll_s: 4,
            hr: Sensor::None,
            speed_cadence: Sensor::None,
            power: Sensor::None,
            follow_route: true,
            auto_off_min: 20,
            trip_log: false,
        }
    }
}

/// Why [`Settings::set`] refused a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    UnknownKey,
    BadValue,
}

/// The keys, in the order the file is written.
pub const KEYS: [&str; 12] = [
    "sound",
    "tz",
    "theme",
    "backlight",
    "usage",
    "scroll_s",
    "hr",
    "speed_cadence",
    "power",
    "follow_route",
    "auto_off_min",
    "trip_log",
];

impl Settings {
    /// Reads a settings file: defaults for what is missing or invalid, unknown keys and other lines ignored.
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let _ = s.set(key.trim(), value.trim());
            }
        }
        s
    }

    /// Sets one setting from its text.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        let bad = Error::BadValue;
        match key {
            "sound" => self.sound = on_off(value).ok_or(bad)?,
            "tz" => self.tz = Tz::parse(value).ok_or(bad)?,
            "theme" => {
                self.theme = match value {
                    "day" => Theme::Day,
                    "night" => Theme::Night,
                    _ => return Err(bad),
                }
            }
            "backlight" => self.backlight = value.parse().ok().filter(|&v| v <= 100).ok_or(bad)?,
            "usage" => {
                self.usage = match value {
                    "explore" => Usage::Explore,
                    "train" => Usage::Train,
                    _ => return Err(bad),
                }
            }
            "scroll_s" => self.scroll_s = value.parse().ok().filter(|&v| (2..=60).contains(&v)).ok_or(bad)?,
            "hr" => self.hr = Sensor::parse(value).ok_or(bad)?,
            "speed_cadence" => self.speed_cadence = Sensor::parse(value).ok_or(bad)?,
            "power" => self.power = Sensor::parse(value).ok_or(bad)?,
            "follow_route" => self.follow_route = on_off(value).ok_or(bad)?,
            "auto_off_min" => self.auto_off_min = value.parse().ok().filter(|&v| v <= 120).ok_or(bad)?,
            "trip_log" => self.trip_log = on_off(value).ok_or(bad)?,
            _ => return Err(Error::UnknownKey),
        }
        Ok(())
    }

    /// Writes the value of one setting.
    pub fn get(&self, key: &str, out: &mut impl Write) -> Result<(), Error> {
        let r = match key {
            "sound" => out.write_str(if self.sound { "on" } else { "off" }),
            "tz" => out.write_str(self.tz.as_str()),
            "theme" => out.write_str(match self.theme {
                Theme::Day => "day",
                Theme::Night => "night",
            }),
            "backlight" => write!(out, "{}", self.backlight),
            "usage" => out.write_str(match self.usage {
                Usage::Explore => "explore",
                Usage::Train => "train",
            }),
            "scroll_s" => write!(out, "{}", self.scroll_s),
            "hr" => write!(out, "{}", self.hr),
            "speed_cadence" => write!(out, "{}", self.speed_cadence),
            "power" => write!(out, "{}", self.power),
            "follow_route" => out.write_str(if self.follow_route { "on" } else { "off" }),
            "auto_off_min" => write!(out, "{}", self.auto_off_min),
            "trip_log" => out.write_str(if self.trip_log { "on" } else { "off" }),
            _ => return Err(Error::UnknownKey),
        };
        r.map_err(|_| Error::BadValue)
    }

    /// The settings file: every key, one `key=value` line each.
    pub fn write(&self, out: &mut impl Write) -> fmt::Result {
        for key in KEYS {
            out.write_str(key)?;
            out.write_char('=')?;
            self.get(key, out).map_err(|_| fmt::Error)?;
            out.write_char('\n')?;
        }
        Ok(())
    }
}

fn on_off(v: &str) -> Option<bool> {
    match v {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// A POSIX time zone rule: standard offset, optional summer time with its `M` start and end rules.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Tz {
    text: [u8; 48],
    len: u8,
    /// Seconds east of UTC (the POSIX rule counts west: `CET-1` is UTC+1).
    std_s: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dst {
    offset_s: i32,
    start: Rule,
    end: Rule,
}

/// `Mm.w.d/time`: day `d` (0 Sunday) of week `w` (5 = last) of month `m`, at `time` local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rule {
    month: u8,
    week: u8,
    weekday: u8,
    time_s: i32,
}

impl fmt::Debug for Tz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl Tz {
    /// Parses `STDoffset[DST[offset],start[/time],end[/time]]` with `M` rules (names alphabetic or `<...>`).
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() || s.len() > 48 || !s.is_ascii() {
            return None;
        }
        let mut p = Parser { s: s.as_bytes(), i: 0 };
        p.name()?;
        let std_s = -p.offset()?;
        let dst = if p.done() {
            None
        } else {
            p.name()?;
            let offset_s = if p.peek() == Some(b',') { std_s + 3600 } else { -p.offset()? };
            p.expect(b',')?;
            let start = p.rule()?;
            p.expect(b',')?;
            let end = p.rule()?;
            Some(Dst { offset_s, start, end })
        };
        if !p.done() {
            return None;
        }
        let mut text = [0; 48];
        text[..s.len()].copy_from_slice(s.as_bytes());
        Some(Self {
            text,
            len: s.len() as u8,
            std_s,
            dst,
        })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len as usize]).unwrap_or("")
    }

    /// Offset from UTC at Unix time `unix`, seconds (UTC+2 is 7200).
    pub fn offset_s(&self, unix: i64) -> i32 {
        let Some(dst) = self.dst else { return self.std_s };
        let year = civil_from_days(unix.div_euclid(86_400)).0;
        // the start is in standard time, the end in summer time
        let start = dst.start.unix(year) - self.std_s as i64;
        let end = dst.end.unix(year) - dst.offset_s as i64;
        let summer = if start < end {
            (start..end).contains(&unix)
        } else {
            // southern hemisphere: summer over the new year
            unix >= start || unix < end
        };
        if summer { dst.offset_s } else { self.std_s }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn done(&self) -> bool {
        self.i == self.s.len()
    }

    fn expect(&mut self, c: u8) -> Option<()> {
        (self.peek() == Some(c)).then(|| self.i += 1)
    }

    fn name(&mut self) -> Option<()> {
        let start = self.i;
        if self.peek() == Some(b'<') {
            while self.peek()? != b'>' {
                self.i += 1;
            }
            self.i += 1;
            return (self.i - start > 3).then_some(());
        }
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        (self.i - start >= 3).then_some(())
    }

    fn number(&mut self) -> Option<i32> {
        let start = self.i;
        let mut v = 0i32;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            v = v.checked_mul(10)?.checked_add((c - b'0') as i32)?;
            self.i += 1;
        }
        (self.i > start).then_some(v)
    }

    /// `[+-]hh[:mm[:ss]]`, seconds.
    fn offset(&mut self) -> Option<i32> {
        let sign = match self.peek() {
            Some(b'-') => {
                self.i += 1;
                -1
            }
            Some(b'+') => {
                self.i += 1;
                1
            }
            _ => 1,
        };
        Some(sign * self.time()?)
    }

    fn time(&mut self) -> Option<i32> {
        let h = self.number()?;
        let mut t = h * 3600;
        for unit in [60, 1] {
            if self.expect(b':').is_none() {
                break;
            }
            t += self.number()? * unit;
        }
        (h <= 167).then_some(t)
    }

    fn rule(&mut self) -> Option<Rule> {
        self.expect(b'M')?;
        let month = self.number()?;
        self.expect(b'.')?;
        let week = self.number()?;
        self.expect(b'.')?;
        let weekday = self.number()?;
        let time_s = if self.expect(b'/').is_some() { self.offset()? } else { 7200 };
        ((1..=12).contains(&month) && (1..=5).contains(&week) && weekday <= 6).then_some(Rule {
            month: month as u8,
            week: week as u8,
            weekday: weekday as u8,
            time_s,
        })
    }
}

impl Rule {
    /// The rule's local time in `year`, as seconds since 1970 (add the offset in force to get UTC).
    fn unix(&self, year: i64) -> i64 {
        let first = days_from_civil(year, self.month as i64, 1);
        // 1970-01-01 was a Thursday
        let first_weekday = (first + 4).rem_euclid(7);
        let mut day = 1 + (self.weekday as i64 - first_weekday).rem_euclid(7) + (self.week as i64 - 1) * 7;
        let days_in_month = days_from_civil(year + self.month as i64 / 12, self.month as i64 % 12 + 1, 1) - first;
        while day > days_in_month {
            day -= 7;
        }
        (first + day - 1) * 86_400 + self.time_s as i64
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Year, month, day of a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;

    fn unix(y: i64, m: i64, d: i64, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60
    }

    #[test]
    fn the_file_round_trips() {
        let mut s = Settings::default();
        s.set("sound", "off").unwrap();
        s.set("tz", "UTC0").unwrap();
        s.set("backlight", "80").unwrap();
        s.set("usage", "train").unwrap();
        s.set("hr", "ble:r:E0:E2:58:63:35:07").unwrap();
        s.set("power", "ant:11:12345:5").unwrap();
        let mut text = String::new();
        s.write(&mut text).unwrap();
        assert_eq!(
            text,
            "sound=off\ntz=UTC0\ntheme=day\nbacklight=80\nusage=train\nscroll_s=4\n\
             hr=ble:r:E0:E2:58:63:35:07\nspeed_cadence=none\npower=ant:11:12345:5\nfollow_route=on\nauto_off_min=20\ntrip_log=off\n"
        );
        assert_eq!(Settings::parse(&text), s);
    }

    #[test]
    fn missing_invalid_and_unknown_keys() {
        let s = Settings::parse("backlight=180\nfuture_key=1\nnot a line\n sound = off \n");
        assert_eq!(s.backlight, 50, "invalid: the default");
        assert!(!s.sound);
        assert_eq!(Settings::parse(""), Settings::default());
        let mut s = Settings::default();
        assert_eq!(s.set("colour", "red"), Err(Error::UnknownKey));
        assert_eq!(s.set("tz", "Paris"), Err(Error::BadValue));
        assert_eq!(s.set("scroll_s", "1"), Err(Error::BadValue));
        for bad in ["ble:x:E0:E2:58:63:35:07", "ble:r:E0:E2:58:63:35", "ble:r:E0:E2:58:63:35:07:00", "ant:120:70000:1", "ant:12345:1", "ant:300:1:1", "hrm"] {
            assert_eq!(s.set("hr", bad), Err(Error::BadValue), "{bad}");
        }
        assert_eq!(s, Settings::default(), "a refused value changes nothing");
    }

    #[test]
    fn paris_summer_time() {
        let tz = Tz::parse(PARIS).unwrap();
        assert_eq!(tz.offset_s(unix(2026, 1, 15, 12, 0)), 3600);
        assert_eq!(tz.offset_s(unix(2026, 7, 15, 12, 0)), 7200);
        // 2026: from Sunday 29 March 01:00 UTC to Sunday 25 October 01:00 UTC
        assert_eq!(tz.offset_s(unix(2026, 3, 29, 0, 59)), 3600);
        assert_eq!(tz.offset_s(unix(2026, 3, 29, 1, 0)), 7200);
        assert_eq!(tz.offset_s(unix(2026, 10, 25, 0, 59)), 7200);
        assert_eq!(tz.offset_s(unix(2026, 10, 25, 1, 0)), 3600);
        // 2027: 28 March and 31 October
        assert_eq!(tz.offset_s(unix(2027, 3, 28, 1, 0)), 7200);
        assert_eq!(tz.offset_s(unix(2027, 10, 31, 0, 59)), 7200);
        assert_eq!(tz.offset_s(unix(2027, 10, 31, 1, 0)), 3600);
    }

    #[test]
    fn other_zones() {
        assert_eq!(Tz::parse("UTC0").unwrap().offset_s(0), 0);
        assert_eq!(Tz::parse("<+03>-3").unwrap().offset_s(0), 3 * 3600);
        // New York: second Sunday of March to first Sunday of November, 02:00 local
        let ny = Tz::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(ny.offset_s(unix(2026, 3, 8, 6, 59)), -5 * 3600);
        assert_eq!(ny.offset_s(unix(2026, 3, 8, 7, 0)), -4 * 3600);
        assert_eq!(ny.offset_s(unix(2026, 11, 1, 5, 59)), -4 * 3600);
        assert_eq!(ny.offset_s(unix(2026, 11, 1, 6, 0)), -5 * 3600);
        // Sydney: summer over the new year
        let syd = Tz::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(syd.offset_s(unix(2026, 1, 15, 0, 0)), 11 * 3600);
        assert_eq!(syd.offset_s(unix(2026, 7, 15, 0, 0)), 10 * 3600);
        for bad in ["", "CET", "CET-1CEST", "CET-1CEST,M3.5.0", "CET-1CEST,M13.5.0,M10.5.0", "CET-1x"] {
            assert_eq!(Tz::parse(bad), None, "{bad:?}");
        }
    }
}
