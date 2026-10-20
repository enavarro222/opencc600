//! ANT+ sensors through the nRF (it is only the radio): the frames it sends fed to [`sensors::ant::Ant`], and what
//! that asks for turned into commands for the main loop to send ([`poll`]).

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use heapless::Vec;
use log::info;
use nrf_link::{Message, command};
use sensors::ant::{Ant, Id, Link, Request};
use settings::Sensor;

use crate::ble;

/// No channel opened before this long after boot: the nRF kills the ones opened within ~6 s of the power-on
/// command, and doing that again and again left its ANT stack silent (c606-oss waits 8 s).
const OPEN_FROM_MS: u64 = 10_000;
/// Wheel circumference until it is a setting (700×25C), as the BLE one.
const WHEEL_MM: u16 = 2105;

static ANT: Mutex<CriticalSectionRawMutex, RefCell<Ant>> = Mutex::new(RefCell::new(Ant::new(OPEN_FROM_MS, WHEEL_MM)));
/// The last state logged of each device type's channel: "not connected" also comes every 5 s, logged when it changes.
static STATES: Mutex<CriticalSectionRawMutex, RefCell<Vec<(u8, u8), 8>>> = Mutex::new(RefCell::new(Vec::new()));
/// Pages logged so far, and when the last one was: the first ones, then one every 10 s.
static PAGES_LOGGED: Mutex<CriticalSectionRawMutex, RefCell<(u32, u64)>> = Mutex::new(RefCell::new((0, 0)));

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

fn id_of(sensor: Sensor) -> Option<Id> {
    match sensor {
        Sensor::Ant { device_type, number, transmission } => Some(Id { device_type, number, transmission }),
        _ => None,
    }
}

fn sensor_of(id: Id) -> Sensor {
    Sensor::Ant { device_type: id.device_type, number: id.number, transmission: id.transmission }
}

/// The paired sensors, from the settings (the BLE ones are left out).
pub fn pair(p: &ble::Paired) {
    let ids = [id_of(p.hr), id_of(p.speed_cadence), id_of(p.power)];
    ANT.lock(|a| a.borrow_mut().pair(ids));
}

/// A frame from the nRF; the ANT+ ones are taken.
pub fn on_message(msg: &Message) {
    let now = now_ms();
    match *msg {
        Message::AntChannel { device_type, number_low, transmission, state } => {
            let changed = STATES.lock(|s| {
                let mut s = s.borrow_mut();
                match s.iter_mut().find(|(t, _)| *t == device_type) {
                    Some((_, last)) if *last == state => false,
                    Some((_, last)) => {
                        *last = state;
                        true
                    }
                    None => {
                        let _ = s.push((device_type, state));
                        true
                    }
                }
            });
            if changed {
                info!("ant: channel {device_type} ({number_low}-{transmission}) state {state}");
            }
            ANT.lock(|a| a.borrow_mut().channel_state(device_type, state, now));
        }
        Message::AntFound { end: true, .. } => info!("ant: search done"),
        Message::AntFound { device_type, number, transmission, rssi_dbm, end: false } => {
            let id = Id { device_type, number, transmission };
            info!("ant: found {} rssi {rssi_dbm} dBm", sensor_of(id));
            ANT.lock(|a| a.borrow_mut().heard(id, rssi_dbm));
        }
        Message::AntPage { device_type, page } => {
            let log = PAGES_LOGGED.lock(|l| {
                let mut l = l.borrow_mut();
                let log = l.0 < 20 || now.saturating_sub(l.1) >= 10_000;
                if log {
                    *l = (l.0 + 1, now);
                }
                log
            });
            if log {
                info!("ant: page {device_type}: {page:02X?}");
            }
            ANT.lock(|a| a.borrow_mut().page(device_type, &page, now));
        }
        _ => {}
    }
}

/// The next command for the nRF, if any (call often: one at a time, spaced out).
pub fn poll() -> Option<[u8; command::CTRL_FRAME]> {
    let request = ANT.lock(|a| a.borrow_mut().poll(now_ms()))?;
    Some(match request {
        Request::Open(id) => {
            info!("ant: open {}", sensor_of(id));
            command::ant_open(id.device_type, id.number, id.transmission)
        }
        Request::Close(t) => {
            info!("ant: close type {t}");
            command::ant_close(t)
        }
    })
}

/// A search for this many seconds (0: stop), its sensors in [`found`]; `fresh`: forget the last search's.
pub fn search(seconds: u16, fresh: bool) -> [u8; command::CTRL_FRAME] {
    if fresh {
        ANT.lock(|a| a.borrow_mut().search_started());
    }
    command::ant_scan(seconds)
}

/// The sensors the last search heard for a role (0 heart rate, 1 speed / cadence, 2 power), closest first.
pub fn found<const N: usize>(role: usize) -> Vec<(Sensor, i8), N> {
    ANT.lock(|a| a.borrow().found(role).take(N).map(|f| (sensor_of(f.id), f.rssi_dbm)).collect())
}

/// Pages are coming from this sensor.
pub fn connected(sensor: &Sensor) -> bool {
    id_of(*sensor).is_some_and(|id| ANT.lock(|a| a.borrow().link_of(id, now_ms())) == Link::Connected)
}

/// The values now, as the BLE ones.
pub fn current() -> ble::Current {
    let v = ANT.lock(|a| a.borrow().values(now_ms()));
    ble::Current { heart_rate: v.heart_rate, cadence_rpm: v.cadence_rpm, speed_mm_s: v.speed_mm_s, power_w: v.power_w }
}

/// The channels with pages coming, for the trip log.
pub fn links() -> usize {
    ANT.lock(|a| {
        let a = a.borrow();
        (0..sensors::ant::ROLES).filter(|&r| a.link(r, now_ms()) == Link::Connected).count()
    })
}
