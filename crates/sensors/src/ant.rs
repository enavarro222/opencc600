//! ANT+ sensors through a radio that only opens channels and hands the data pages over (the nRF of the CC600,
//! c606-oss `main/ant.c`): the pages decoded, and [`Ant`], what to ask the radio and the values now.

use crate::{Calibration, Revs};
use crate::ble::{Csc, RevEvent};

/// ANT+ device types.
pub const POWER: u8 = 11;
pub const HEART_RATE: u8 = 120;
pub const SPEED_CADENCE: u8 = 121;
pub const CADENCE: u8 = 122;
pub const SPEED: u8 = 123;

/// The roles of the sensors, in this order: heart rate, speed / cadence, power.
pub const ROLES: usize = 3;

/// The role of a device type ([`ROLES`] index), `None` for the types we do not use.
pub fn role(device_type: u8) -> Option<usize> {
    match device_type {
        HEART_RATE => Some(0),
        SPEED_CADENCE | CADENCE | SPEED => Some(1),
        POWER => Some(2),
        _ => None,
    }
}

/// An ANT+ sensor: its device type, device number and transmission type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Id {
    pub device_type: u8,
    pub number: u16,
    pub transmission: u8,
}

/// Heart rate, any page: the computed rate in byte 7 (`None`: 0, no contact).
pub fn heart_rate(page: &[u8; 8]) -> Option<u8> {
    Some(page[7]).filter(|&b| b > 0)
}

/// Speed / cadence sensors, any page: the event time (1/1024 s) then the revolutions, 16 bits each, in bytes 4..8
/// (122 cadence, 123 speed); the combined 121 has the crank in bytes 0..4 and the wheel in 4..8. The counts wrap
/// at 16 bits (unlike BLE's 32-bit wheel count).
pub fn speed_cadence(device_type: u8, page: &[u8; 8]) -> Csc {
    let event = |i: usize| RevEvent {
        time: u16::from_le_bytes([page[i], page[i + 1]]),
        revs: u16::from_le_bytes([page[i + 2], page[i + 3]]) as u32,
    };
    match device_type {
        CADENCE => Csc { wheel: None, crank: Some(event(4)) },
        SPEED => Csc { wheel: Some(event(4)), crank: None },
        SPEED_CADENCE => Csc { wheel: Some(event(4)), crank: Some(event(0)) },
        _ => Csc::default(),
    }
}

/// Power meter, page 0x10 (standard power only): the instant power and the cadence if the meter has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    pub watts: u16,
    pub cadence_rpm: Option<u8>,
}

/// `None` for the other pages (torque, calibration, battery...).
pub fn power(page: &[u8; 8]) -> Option<Power> {
    (page[0] == 0x10).then(|| Power {
        watts: u16::from_le_bytes([page[6], page[7]]),
        cadence_rpm: Some(page[3]).filter(|&c| c != 0xFF),
    })
}

/// Power meter, page 0x01: the calibration request "manual zero" (sent to the meter, [`Ant::calibrate`]).
pub const CALIBRATION_REQUEST: [u8; 8] = [0x01, 0xAA, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Power meter, page 0x01: the answer to [`CALIBRATION_REQUEST`], 0xAC success (the offset in bytes 6..8) or 0xAF
/// failure; `None` for the other pages.
pub fn calibration(page: &[u8; 8]) -> Option<Calibration> {
    match page[..2] {
        [0x01, 0xAC] => Some(Calibration::Done(i16::from_le_bytes([page[6], page[7]]))),
        [0x01, 0xAF] => Some(Calibration::Failed),
        _ => None,
    }
}

/// What [`Ant::poll`] asks the radio for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open a channel to this sensor (the radio searches for it, then says connected or search timeout).
    Open(Id),
    /// Close the channel of this device type (another sensor paired in its place, or none).
    Close(u8),
    /// Send this page to the sensor of this device type.
    Send(u8, [u8; 8]),
}

/// A role's link, for the screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// No sensor paired.
    None,
    Searching,
    /// Pages coming.
    Connected,
}

/// The values now (`None`: no sensor, or nothing for [`Ant::STALE_MS`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Values {
    pub heart_rate: Option<u8>,
    pub cadence_rpm: Option<u8>,
    /// From the wheel sensor.
    pub speed_mm_s: Option<u32>,
    pub power_w: Option<u16>,
}

/// A sensor heard by a search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub id: Id,
    pub rssi_dbm: i8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// To open.
    Closed,
    /// Open asked at this time, no answer yet.
    Asked(u64),
    /// The radio is searching for it, since this time.
    Searching(u64),
    Connected,
}

#[derive(Clone, Copy, Debug)]
struct Channel {
    id: Option<Id>,
    state: State,
    last_page_ms: Option<u64>,
    crank: Revs,
    wheel: Revs,
    // a value and when it came
    heart_rate: Option<(u8, u64)>,
    cadence: Option<(u16, u64)>,
    speed_mm_s: Option<(u32, u64)>,
    power: Option<(u16, u64)>,
    power_cadence: Option<(u8, u64)>,
}

impl Channel {
    const fn new(id: Option<Id>) -> Self {
        Self {
            id,
            state: State::Closed,
            last_page_ms: None,
            crank: Revs { last: None, rate: None },
            wheel: Revs { last: None, rate: None },
            heart_rate: None,
            cadence: None,
            speed_mm_s: None,
            power: None,
            power_cadence: None,
        }
    }

    fn fed_since(&self, now_ms: u64, ms: u64) -> bool {
        self.last_page_ms.is_some_and(|t| now_ms.saturating_sub(t) < ms)
    }
}

/// Found sensors kept from a search.
pub const FOUND_MAX: usize = 8;

/// The ANT+ sensors: the channels of the paired ones, kept open, their pages decoded; the last search's sensors.
///
/// What the radio does, from c606-oss: it keeps its channels when the ESP32 restarts and reports each every 5 s
/// (connected or not connected), so a channel found connected is tracked, not reopened, and "not connected" alone
/// is no reason to reopen one (that drops a live channel). A channel is opened again after a search timeout. No
/// open before `open_from_ms` (the radio kills channels opened in the first seconds, and doing that again and again
/// left its ANT stack silent).
#[derive(Clone, Debug)]
pub struct Ant {
    channels: [Channel; ROLES],
    open_from_ms: u64,
    wheel_mm: u16,
    closes: [Option<u8>; ROLES],
    last_request_ms: Option<u64>,
    found: [Option<Found>; FOUND_MAX],
    /// A page to send to a sensor, after the closes.
    send: Option<(u8, [u8; 8])>,
    calibration: Option<Calibration>,
}

impl Ant {
    /// A value older than this is gone.
    pub const STALE_MS: u64 = 5_000;
    /// Between two requests to the radio (c606-oss: 50 ms between opens).
    pub const REQUEST_GAP_MS: u64 = 100;
    /// An open never answered is asked again after this (the radio answers within its 25 s search).
    pub const ASK_AGAIN_MS: u64 = 40_000;
    /// Still searching after this long: opened again. The CC600's radio never says its search timed out (the
    /// C606's does after 25 s): in case its search stopped without a word.
    pub const SEARCH_AGAIN_MS: u64 = 120_000;
    /// A connected channel without a page for this long is opened again.
    pub const LOST_MS: u64 = 30_000;
    /// A "not connected" report with no page for this long: the sensor is gone (searching).
    const QUIET_MS: u64 = 3_000;

    pub const fn new(open_from_ms: u64, wheel_mm: u16) -> Self {
        Self {
            channels: [Channel::new(None); ROLES],
            open_from_ms,
            wheel_mm,
            closes: [None; ROLES],
            last_request_ms: None,
            found: [None; FOUND_MAX],
            send: None,
            calibration: None,
        }
    }

    pub fn set_wheel_mm(&mut self, mm: u16) {
        self.wheel_mm = mm;
    }

    /// The paired sensors, by role (`None`: none, or not an ANT+ one). A sensor replaced has its channel closed.
    pub fn pair(&mut self, ids: [Option<Id>; ROLES]) {
        for (c, id) in self.channels.iter_mut().zip(ids) {
            if c.id == id {
                continue;
            }
            if let Some(old) = c.id
                && c.state != State::Closed
                && let Some(slot) = self.closes.iter_mut().find(|s| s.is_none())
            {
                *slot = Some(old.device_type);
            }
            *c = Channel::new(id);
        }
    }

    fn channel_of(&mut self, device_type: u8) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id.is_some_and(|id| id.device_type == device_type))
    }

    /// A channel state from the radio (`nrf_link::ant` states: 3 connected, 4 not connected, 5 search timeout).
    pub fn channel_state(&mut self, device_type: u8, state: u8, now_ms: u64) {
        let Some(c) = self.channel_of(device_type) else {
            return;
        };
        match state {
            3 => c.state = State::Connected,
            5 => c.state = State::Closed,
            4 => match c.state {
                State::Asked(_) => c.state = State::Searching(now_ms),
                State::Connected if !c.fed_since(now_ms, Self::QUIET_MS) => c.state = State::Searching(now_ms),
                _ => {}
            },
            _ => {}
        }
    }

    /// A data page from the radio.
    pub fn page(&mut self, device_type: u8, page: &[u8; 8], now_ms: u64) {
        let wheel_mm = self.wheel_mm;
        if device_type == POWER
            && let Some(c) = calibration(page)
        {
            self.calibration = Some(c);
        }
        let Some(c) = self.channel_of(device_type) else {
            return;
        };
        c.state = State::Connected;
        c.last_page_ms = Some(now_ms);
        match device_type {
            HEART_RATE => {
                if let Some(bpm) = heart_rate(page) {
                    c.heart_rate = Some((bpm, now_ms));
                }
            }
            SPEED_CADENCE | CADENCE | SPEED => {
                let m = speed_cadence(device_type, page);
                if let Some(e) = m.crank {
                    c.crank.update(e.revs, e.time, u16::MAX as u32, now_ms);
                    if let Some(rpm) = c.crank.rpm() {
                        c.cadence = Some((rpm, now_ms));
                    }
                }
                if let Some(e) = m.wheel {
                    c.wheel.update(e.revs, e.time, u16::MAX as u32, now_ms);
                    if let Some(v) = c.wheel.speed_mm_s(wheel_mm) {
                        c.speed_mm_s = Some((v, now_ms));
                    }
                }
            }
            POWER => {
                if let Some(p) = power(page) {
                    c.power = Some((p.watts, now_ms));
                    if let Some(rpm) = p.cadence_rpm {
                        c.power_cadence = Some((rpm, now_ms));
                    }
                }
            }
            _ => {}
        }
    }

    /// What to ask the radio now, one request at a time ([`Self::REQUEST_GAP_MS`] apart): the closes, then the
    /// channels to open.
    pub fn poll(&mut self, now_ms: u64) -> Option<Request> {
        if self.last_request_ms.is_some_and(|t| now_ms.saturating_sub(t) < Self::REQUEST_GAP_MS) {
            return None;
        }
        let request = self.next_request(now_ms)?;
        self.last_request_ms = Some(now_ms);
        Some(request)
    }

    fn next_request(&mut self, now_ms: u64) -> Option<Request> {
        if let Some(t) = self.closes.iter_mut().find_map(Option::take) {
            return Some(Request::Close(t));
        }
        if let Some((t, page)) = self.send.take() {
            return Some(Request::Send(t, page));
        }
        if now_ms < self.open_from_ms {
            return None;
        }
        for c in &mut self.channels {
            let Some(id) = c.id else { continue };
            let open = match c.state {
                State::Closed => true,
                State::Asked(at) => now_ms.saturating_sub(at) >= Self::ASK_AGAIN_MS,
                State::Searching(since) => now_ms.saturating_sub(since) >= Self::SEARCH_AGAIN_MS,
                // pages stopped long ago: whatever the radio says, open it again
                State::Connected => !c.fed_since(now_ms, Self::LOST_MS) && c.last_page_ms.is_some(),
            };
            if open {
                c.state = State::Asked(now_ms);
                c.last_page_ms = None;
                return Some(Request::Open(id));
            }
        }
        None
    }

    /// Asks the power meter for a zero offset calibration; `false` if it is not connected. Its answer:
    /// [`Self::calibrated`].
    pub fn calibrate(&mut self, now_ms: u64) -> bool {
        let Some(id) = self.channels[2].id.filter(|_| self.link(2, now_ms) == Link::Connected) else {
            return false;
        };
        self.calibration = None;
        self.send = Some((id.device_type, CALIBRATION_REQUEST));
        true
    }

    /// The power meter's answer to the last [`Self::calibrate`], once.
    pub fn calibrated(&mut self) -> Option<Calibration> {
        self.calibration.take()
    }

    /// The link of a role's sensor.
    pub fn link(&self, role: usize, now_ms: u64) -> Link {
        match self.channels.get(role) {
            None | Some(Channel { id: None, .. }) => Link::None,
            Some(c) if c.state == State::Connected && c.fed_since(now_ms, Self::STALE_MS) => Link::Connected,
            Some(_) => Link::Searching,
        }
    }

    /// The link of a paired sensor, [`Link::None`] if it is not paired.
    pub fn link_of(&self, id: Id, now_ms: u64) -> Link {
        match self.channels.iter().position(|c| c.id == Some(id)) {
            Some(role) => self.link(role, now_ms),
            None => Link::None,
        }
    }

    /// The values now.
    pub fn values(&self, now_ms: u64) -> Values {
        fn fresh<T: Copy>(v: Option<(T, u64)>, now_ms: u64) -> Option<T> {
            v.filter(|&(_, at)| now_ms.saturating_sub(at) < Ant::STALE_MS).map(|(v, _)| v)
        }
        let [hr, sc, pm] = &self.channels;
        Values {
            heart_rate: fresh(hr.heart_rate, now_ms),
            // the power meter's, else the speed / cadence sensor's
            cadence_rpm: fresh(pm.power_cadence, now_ms)
                .or(fresh(sc.cadence, now_ms).map(|c| c.min(254) as u8)),
            speed_mm_s: fresh(sc.speed_mm_s, now_ms),
            power_w: fresh(pm.power, now_ms),
        }
    }

    /// A search starts: the last one's sensors are forgotten.
    pub fn search_started(&mut self) {
        self.found = [None; FOUND_MAX];
    }

    /// A sensor heard by the search (the types we do not use are left out). When full, the weakest goes.
    pub fn heard(&mut self, id: Id, rssi_dbm: i8) {
        if role(id.device_type).is_none() || id.number == 0 {
            return;
        }
        let slot = match self.found.iter().position(|f| f.is_some_and(|f| f.id == id)) {
            Some(i) => i,
            None => match self.found.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    let (i, weakest) = self
                        .found
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, f)| f.map_or(i8::MIN, |f| f.rssi_dbm))
                        .unwrap();
                    if weakest.is_some_and(|w| w.rssi_dbm >= rssi_dbm) {
                        return;
                    }
                    i
                }
            },
        };
        self.found[slot] = Some(Found { id, rssi_dbm });
    }

    /// The sensors the last search heard for a role, closest first.
    pub fn found(&self, role_index: usize) -> impl Iterator<Item = Found> + '_ {
        let mut all = self.found;
        all.sort_unstable_by_key(|f| core::cmp::Reverse(f.map_or(i8::MIN, |f| f.rssi_dbm)));
        all.into_iter().flatten().filter(move |f| role(f.id.device_type) == Some(role_index))
    }
}
