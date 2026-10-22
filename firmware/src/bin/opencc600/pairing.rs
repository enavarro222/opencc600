//! The sensors (BLE and ANT+): what the sensors screens show, the pairing, the power meter's calibration, the
//! values now.

use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant};
use log::info;
use opencc600_firmware::{ant, ble};

use crate::config::{SETTINGS, SETTINGS_CHANGED, settings};

/// The role whose sensors are being looked for (the scan screen), as a `ble::ROLE_*` bit.
pub(crate) static SCAN_ROLE: Mutex<CriticalSectionRawMutex, Cell<Option<u8>>> = Mutex::new(Cell::new(None));

/// The sensors the scan screen shows, in its order: what [`ui::Command::Pair`] picks from. Set when the screen is
/// drawn: [`sensors_view`] sorts the list again each time, by signal strength, and the order changes often.
pub(crate) static SHOWN_FOUND: Mutex<CriticalSectionRawMutex, Cell<[Option<settings::Sensor>; ui::FOUND]>> =
    Mutex::new(Cell::new([None; ui::FOUND]));

/// The list of the last [`sensors_view`], for [`SHOWN_FOUND`] once drawn.
pub(crate) static LISTED_FOUND: Mutex<CriticalSectionRawMutex, Cell<[Option<settings::Sensor>; ui::FOUND]>> =
    Mutex::new(Cell::new([None; ui::FOUND]));

pub(crate) const ROLE_BITS: [u8; 3] = [ble::ROLE_HR, ble::ROLE_SPEED_CADENCE, ble::ROLE_POWER];

pub(crate) fn role_index(role: ui::Role) -> usize {
    ui::Role::ALL.iter().position(|&r| r == role).unwrap_or(0)
}

/// The paired sensor of each role, in [`ui::Role::ALL`] order.
pub(crate) fn role_sensors(s: &settings::Settings) -> [settings::Sensor; 3] {
    [s.hr, s.speed_cadence, s.power]
}

/// Pairs a role with a sensor (or none): saved, and the BLE task told by the main loop.
pub(crate) fn set_sensor(role: ui::Role, sensor: settings::Sensor) {
    let mut s = settings();
    match role {
        ui::Role::HeartRate => s.hr = sensor,
        ui::Role::SpeedCadence => s.speed_cadence = sensor,
        ui::Role::Power => s.power = sensor,
    }
    info!("sensors: {role:?} = {sensor}");
    SETTINGS.lock(|c| c.set(Some(s)));
    SETTINGS_CHANGED.signal(());
}

/// What names a sensor on the screens, after its radio (a dual strap is listed twice): the name it gave, else the
/// end of its address or its ANT number.
pub(crate) fn sensor_name(sensor: &settings::Sensor) -> ui::fonts::Line {
    use settings::Sensor;
    match (ble::name_of(sensor), sensor) {
        (Some(n), _) => ui::fonts::Line::new(format_args!("BT {}", n.as_str())),
        (None, Sensor::Ble { addr, .. }) => ui::fonts::Line::new(format_args!("BT …{:02X}{:02X}", addr[1], addr[0])),
        (None, Sensor::Ant { device_type, number, .. }) => {
            let kind = match *device_type {
                sensors::ant::SPEED_CADENCE => " V+C",
                sensors::ant::CADENCE => " cadence",
                sensors::ant::SPEED => " vitesse",
                _ => "",
            };
            ui::fonts::Line::new(format_args!("ANT+ {number}{kind}"))
        }
        (None, Sensor::None) => ui::fonts::Line::new(format_args!("–")),
    }
}

/// The sensors screens' content: each role's sensor and link, and what the scan found for the role looked for.
#[allow(
    clippy::large_stack_frames,
    reason = "the names (64-byte lines) and the scan list, from the main loop (215 KB stack)"
)]
pub(crate) fn sensors_view() -> ui::Sensors {
    let paired = role_sensors(&settings());
    let mut out = ui::Sensors::default();
    for (row, sensor) in out.roles.iter_mut().zip(paired.iter()) {
        if *sensor != settings::Sensor::None {
            *row = ui::SensorRow {
                link: if ble::connected(sensor) || ant::connected(sensor) {
                    ui::Link::Connected
                } else {
                    ui::Link::Searching
                },
                name: Some(sensor_name(sensor)),
            };
        }
    }
    let mut shown = [None; ui::FOUND];
    if let Some(bit) = SCAN_ROLE.lock(Cell::get) {
        let role = ROLE_BITS.iter().position(|&b| b == bit).unwrap_or(0);
        // BLE and ANT+ together, closest first
        let mut all: heapless::Vec<(settings::Sensor, i8), { 2 * ui::FOUND }> =
            ble::found::<{ ui::FOUND }>(bit).into_iter().map(|h| (h.sensor, h.rssi_dbm)).collect();
        all.extend(ant::found::<{ ui::FOUND }>(role));
        all.sort_unstable_by_key(|&(_, rssi)| core::cmp::Reverse(rssi));
        for (i, (sensor, rssi_dbm)) in all.into_iter().take(ui::FOUND).enumerate() {
            shown[i] = Some(sensor);
            out.found[i] = Some(ui::Found {
                name: sensor_name(&sensor),
                rssi_dbm,
                paired: sensor == paired[role],
            });
        }
    }
    LISTED_FOUND.lock(|c| c.set(shown));
    out.calibration = power_calibration();
    out
}

/// The power meter's calibration, and when it was asked.
pub(crate) static CALIBRATION: Mutex<CriticalSectionRawMutex, Cell<(ui::Calibration, Instant)>> =
    Mutex::new(Cell::new((ui::Calibration::None, Instant::from_ticks(0))));

/// A calibration without an answer after this long failed (the meters answer within a few seconds).
pub(crate) const CALIBRATION_WAIT: Duration = Duration::from_secs(20);

/// Asks the paired power meter, over its radio, for a zero offset calibration.
pub(crate) fn calibrate_power() {
    let sensor = settings().power;
    let state = match sensor {
        settings::Sensor::Ble { .. } if ble::calibrate() => ui::Calibration::Running,
        settings::Sensor::Ble { .. } if ble::connected(&sensor) => ui::Calibration::Unsupported,
        settings::Sensor::Ant { .. } if ant::calibrate() => ui::Calibration::Running,
        _ => ui::Calibration::NotConnected,
    };
    info!("power meter calibration: {state:?}");
    CALIBRATION.lock(|c| c.set((state, Instant::now())));
}

/// The power meter calibration now: its answer, or no answer in time.
pub(crate) fn power_calibration() -> ui::Calibration {
    let (mut state, at) = CALIBRATION.lock(Cell::get);
    if state == ui::Calibration::Running {
        if let Some(answer) = ble::calibrated().or_else(ant::calibrated) {
            state = match answer {
                sensors::Calibration::Done(offset) => ui::Calibration::Done(offset),
                sensors::Calibration::Failed => ui::Calibration::Failed,
                sensors::Calibration::Unsupported => ui::Calibration::Unsupported,
            };
        } else if at.elapsed() >= CALIBRATION_WAIT {
            state = ui::Calibration::Timeout;
        }
        if state != ui::Calibration::Running {
            info!("power meter calibration: {state:?}");
            CALIBRATION.lock(|c| c.set((state, at)));
        }
    }
    state
}

/// The power of the last 3 s, fed by each frame drawn.
pub(crate) static POWER_3S: Mutex<CriticalSectionRawMutex, Cell<sensors::Avg3s>> = Mutex::new(Cell::new(sensors::Avg3s::new()));

/// The sensor values now: the BLE ones, else the ANT+ ones.
pub(crate) fn sensor_values() -> ble::Current {
    let (b, a) = (ble::current(), ant::current());
    ble::Current {
        heart_rate: b.heart_rate.or(a.heart_rate),
        cadence_rpm: b.cadence_rpm.or(a.cadence_rpm),
        speed_mm_s: b.speed_mm_s.or(a.speed_mm_s),
        power_w: b.power_w.or(a.power_w),
    }
}

/// The paired sensors of the settings, for the BLE task.
pub(crate) fn paired(s: &settings::Settings) -> ble::Paired {
    ble::Paired {
        hr: s.hr,
        speed_cadence: s.speed_cadence,
        power: s.power,
    }
}
