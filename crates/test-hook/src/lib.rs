//! The test hook: lines typed on the USB serial port (the firmware keeps writing its log on the other side), by
//! the HIL tests (`tests/hil/dut.py`) and the tools (`serial_log.py`, `send_route.py`, `replay_gpx.py`).
//!
//! A line is `[<seq>] <command>`. With a sequence number the firmware logs `usb ack <seq>`: the link loses ~5 % of the
//! received lines when the log is busy, so the host resends until acknowledged; a repeated number is acknowledged
//! again but not run twice.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use nrf_link::KeyAction;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command<'a> {
    /// `key <0-2> <click|long>`: key presses as the nRF sends them (`long` = long press then release).
    Key(u8, &'static [KeyAction]),
    /// `sound <on|off>`: the buzzer, until the next boot.
    Sound(bool),
    /// `gnss <hot|warm|cold|full>`: restarts the receiver (time-to-first-fix measurements).
    GnssStart(gnss::Start),
    /// `gnss time <on|off>`: answers the receiver's time requests with the RTC time (`$PAIR590`).
    GnssTimeAiding(bool),
    /// `ble scan <seconds>`: logs the BLE cycling sensors around (`ble: found ble:r:… "name"`), for `set hr …`.
    BleScan(u16),
    /// `nrf e2 GRP ID VAL`: sends `E2 GRP ID 00 00 VAL 00 00` to the nRF (exploring its commands).
    NrfE2(u8, u8, u8),
    /// `nrf raw KIND CMD B0 .. B7` (hex): any frame with an 8-byte payload.
    NrfRaw(u8, u8, [u8; 8]),
    /// `ant scan <seconds>`: an ANT+ search by the nRF (0 stops), the sensors logged (`ant: found ant:120:…`).
    AntScan(u16),
    /// `battery fake N`: the battery level shown, not the one measured (the charging screen's animation at full
    /// charge); `battery real` (`None`) the measured one again.
    FakeBattery(Option<u8>),
    /// `power off dry`: until the next boot, a power off restarts the ESP32 instead of asking the nRF to cut it, which
    /// would stop the nRF's sensor stream (time, barometer) until a long press of the power key.
    DryPowerOff,
    /// `gnss off [N]`: switches the receiver off through the nRF, as it was found at a boot, and drops the firmware's
    /// next N power-ons (lost, as that day): the firmware must keep powering it on.
    GnssOff(u8),
    /// `rtc shift <s>`: moves the nRF RTC by this many seconds, to check that a fix sets it right again.
    RtcShift(i32),
    /// `screen ?`: logs the screen shown (`screen Home`), for the HIL tests.
    Screen,
    /// `set key value`: changes a setting (saved); `get key`, `settings ?`: logs them.
    Set(&'a str, &'a str),
    Get(&'a str),
    Settings,
    /// `route begin <bytes>`, `route data <hex><crc>`..., `route end`: a route file sent from the PC
    /// (`tools/send_route.py`); `route clear` removes it.
    RouteBegin(usize),
    /// The hex of a `route data` line whose CRC matched (the CRC removed).
    RouteData(&'a str),
    /// A `route data` line whose CRC did not match: the link also loses bytes inside a line. Not acknowledged, so
    /// the host sends it again.
    RouteDataDamaged,
    RouteEnd,
    RouteClear,
    /// `bench map`: draws the map page on the route at each zoom and logs the times.
    BenchMap,
    /// `storage fill N`: the logs deleted, the storage filled until N blocks are left free; `storage unfill`
    /// (`None`): emptied again.
    StorageFill(Option<u16>),
    /// `light <raw>` in place of the ambient light sensor for the automatic backlight; `light off` (`None`) gives it
    /// back.
    SimLight(Option<u16>),
    /// `light ?`: logs the theme shown and the backlight (`light: theme night, backlight 10 %`).
    Light,
    /// `fix <lat_e7> <lon_e7> [<speed_mm_s> [<course_cdeg>]]`: a position in place of the receiver's for a few
    /// seconds (tools/replay_gpx.py); `fix off` (`None`) gives the receiver back at once.
    SimFix(Option<SimFix>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimFix {
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub speed_mm_s: Option<u32>,
    pub course_cdeg: Option<u16>,
}

/// `<seq> <command>` → (seq, command); a line without a number is a command typed by hand.
pub fn split_seq(line: &str) -> (Option<u32>, &str) {
    let line = line.trim();
    match line.split_once(' ') {
        Some((n, rest)) if n.bytes().all(|b| b.is_ascii_digit()) => (n.parse().ok(), rest),
        _ => (None, line),
    }
}

/// The command of a line (without its sequence number); `None` if unknown or malformed.
pub fn parse(line: &str) -> Option<Command<'_>> {
    let mut words = line.split_whitespace();
    match (words.next()?, words.next()?, words.next()) {
        ("sound", "on", None) => Some(Command::Sound(true)),
        ("sound", "off", None) => Some(Command::Sound(false)),
        ("gnss", "time", Some(on @ ("on" | "off"))) => Some(Command::GnssTimeAiding(on == "on")),
        ("power", "off", Some("dry")) => Some(Command::DryPowerOff),
        ("battery", "real", None) => Some(Command::FakeBattery(None)),
        ("battery", "fake", Some(p)) => Some(Command::FakeBattery(Some(p.parse().ok().filter(|&p| p <= 100)?))),
        ("nrf", "e2", Some(grp)) => {
            let id = words.next()?.parse().ok()?;
            let val = words.next()?.parse().ok()?;
            Some(Command::NrfE2(grp.parse().ok()?, id, val))
        }
        ("nrf", "raw", Some(kind)) => {
            let hex = |w: &str| u8::from_str_radix(w, 16).ok();
            let cmd = hex(words.next()?)?;
            let mut payload = [0; 8];
            for b in &mut payload {
                *b = hex(words.next()?)?;
            }
            Some(Command::NrfRaw(hex(kind)?, cmd, payload))
        }
        ("ant", "scan", Some(s)) => Some(Command::AntScan(s.parse().ok()?)),
        ("gnss", "off", lost) => Some(Command::GnssOff(lost.map(str::parse).transpose().ok()?.unwrap_or(0))),
        ("gnss", start, None) => Some(Command::GnssStart(match start {
            "hot" => gnss::Start::Hot,
            "warm" => gnss::Start::Warm,
            "cold" => gnss::Start::Cold,
            "full" => gnss::Start::FullCold,
            _ => return None,
        })),
        ("rtc", "shift", Some(s)) => Some(Command::RtcShift(s.parse().ok()?)),
        ("screen", "?", None) => Some(Command::Screen),
        ("set", key, Some(value)) => Some(Command::Set(key, value)),
        ("get", key, None) => Some(Command::Get(key)),
        ("settings", "?", None) => Some(Command::Settings),
        ("route", "begin", Some(n)) => Some(Command::RouteBegin(n.parse().ok()?)),
        ("route", "data", Some(hex)) => Some(route_line(hex).map_or(Command::RouteDataDamaged, Command::RouteData)),
        ("route", "end", None) => Some(Command::RouteEnd),
        ("route", "clear", None) => Some(Command::RouteClear),
        ("bench", "map", None) => Some(Command::BenchMap),
        ("storage", "fill", Some(n)) => Some(Command::StorageFill(Some(n.parse().ok()?))),
        ("storage", "unfill", None) => Some(Command::StorageFill(None)),
        ("light", "?", None) => Some(Command::Light),
        ("light", "off", None) => Some(Command::SimLight(None)),
        ("light", raw, None) => Some(Command::SimLight(Some(raw.parse().ok()?))),
        ("ble", "scan", Some(s)) => Some(Command::BleScan(s.parse().ok().filter(|&s| s > 0)?)),
        ("fix", "off", None) => Some(Command::SimFix(None)),
        ("fix", lat, Some(lon)) => {
            let speed_mm_s = words.next().map(str::parse).transpose().ok()?;
            let course_cdeg = words.next().map(str::parse).transpose().ok()?;
            Some(Command::SimFix(Some(SimFix {
                lat_e7: lat.parse().ok()?,
                lon_e7: lon.parse().ok()?,
                speed_mm_s,
                course_cdeg,
            })))
        }
        ("key", key, Some(action)) => {
            let key = key.parse().ok().filter(|&k: &u8| k <= 2)?;
            let actions: &[KeyAction] = match action {
                "click" => &[KeyAction::Click],
                "long" => &[KeyAction::LongPress, KeyAction::LongRelease],
                _ => return None,
            };
            Some(Command::Key(key, actions))
        }
        _ => None,
    }
}

/// The data of a `route data` line (hex, then the CRC-16 of the bytes in 4 hex digits) if its CRC matches.
fn route_line(hex: &str) -> Option<&str> {
    let (data, crc) = hex.split_at_checked(hex.len().checked_sub(4)?)?;
    let mut bytes = Vec::with_capacity(data.len() / 2);
    (unhex(data, &mut bytes) && u16::from_str_radix(crc, 16).ok()? == route::crc16(&bytes)).then_some(data)
}

/// `hex` appended to `buf`; false if it is not hex.
pub fn unhex(hex: &str, buf: &mut Vec<u8>) -> bool {
    let b = hex.as_bytes();
    if !b.len().is_multiple_of(2) {
        return false;
    }
    let digit = |c: u8| (c as char).to_digit(16);
    for pair in b.chunks(2) {
        match (digit(pair[0]), digit(pair[1])) {
            (Some(h), Some(l)) => buf.push((h * 16 + l) as u8),
            _ => return false,
        }
    }
    true
}
