//! The remote (crates/remote, `ble::remote_link`): the `/remote` file (address, bond, learned buttons), pairing it
//! from the scan screen, learning its buttons, its presses as key presses.

use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use log::{info, warn};
use opencc600_firmware::ble::{self, remote_link};
use remote::{Learned, Learner, Paired};

use alloc::string::String;

use crate::Fs;

pub(crate) const REMOTE_FILE: &str = "/remote";

/// The paired remote, from `/remote`.
static PAIRED: Mutex<CriticalSectionRawMutex, Cell<Option<Paired>>> = Mutex::new(Cell::new(None));

/// Learning its buttons ([`ui::Screen::RemoteLearn`]), and the key of the last press refused.
static LEARNING: Mutex<CriticalSectionRawMutex, Cell<Option<(Learner, Option<u8>)>>> = Mutex::new(Cell::new(None));

pub(crate) fn paired() -> Option<Paired> {
    PAIRED.lock(Cell::get)
}

/// At boot: the remote of `/remote`, its bond for the BLE stack (given with the next [`ble::PAIRED`]).
#[allow(clippy::large_stack_frames, reason = "the file read in a 1 KB buffer, once at boot from main")]
pub(crate) fn load(fs: &Fs<'_>) {
    let path = littlefs2::path::PathBuf::try_from(REMOTE_FILE).unwrap();
    let p = fs
        .read::<1024>(&path)
        .ok()
        .and_then(|data| Paired::parse(core::str::from_utf8(&data).unwrap_or("")));
    match &p {
        Some(p) => info!(
            "remote: {} ({:02X?}), bond {}, {} buttons",
            p.name.as_str(),
            p.address.addr,
            p.bond.is_some(),
            p.buttons.iter().count()
        ),
        None => info!("remote: none"),
    }
    remote_link::BOND.lock(|c| c.set(p.and_then(|p| p.bond)));
    PAIRED.lock(|c| c.set(p));
}

/// Saved in one commit (none: the file removed).
fn save(fs: &Fs<'_>, p: Option<&Paired>) {
    let path = littlefs2::path::PathBuf::try_from(REMOTE_FILE).unwrap();
    let Some(p) = p else {
        let _ = fs.remove(&path);
        info!("remote: forgotten");
        return;
    };
    let mut text = String::new();
    let _ = p.write(&mut text);
    match fs.write(&path, text.as_bytes()) {
        Ok(()) => info!("remote: saved"),
        Err(e) => warn!("remote: save failed, littlefs error {}", e.code()),
    }
}

fn set(fs: Option<&Fs<'_>>, p: Option<Paired>) {
    PAIRED.lock(|c| c.set(p));
    if let Some(fs) = fs {
        save(fs, p.as_ref());
    }
}

/// The remote's address for the BLE task.
pub(crate) fn address() -> Option<remote::Address> {
    paired().map(|p| p.address)
}

/// Pairs this device found by the scan as the remote, and starts learning its buttons. The same remote again keeps
/// its bond and its buttons (until learned again); `true` if the BLE task must be told (another remote).
pub(crate) fn pair(fs: Option<&Fs<'_>>, sensor: settings::Sensor) -> bool {
    let settings::Sensor::Ble { random, addr } = sensor else { return false };
    let address = remote::Address { addr, random };
    let old = paired().filter(|p| p.address == address);
    let name = ble::name_of(&sensor).map(|n| remote::Name::new(n.as_bytes())).unwrap_or_default();
    let p = Paired { address, name, ..old.unwrap_or_default() };
    info!("remote: paired {} ({:02X?}){}", p.name.as_str(), addr, if old.is_some() { ", again" } else { "" });
    set(fs, Some(p));
    LEARNING.lock(|c| c.set(Some((Learner::new(p.buttons), None))));
    if old.is_none() {
        remote_link::BOND.lock(|c| c.set(None));
    }
    old.is_none()
}

/// No remote any more; `true`: the BLE task must be told.
pub(crate) fn forget(fs: Option<&Fs<'_>>) -> bool {
    let had = paired().is_some();
    set(fs, None);
    remote_link::BOND.lock(|c| c.set(None));
    LEARNING.lock(|c| c.set(None));
    had
}

/// A pairing left a bond: kept for the next boots.
pub(crate) fn bonded(fs: Option<&Fs<'_>>, bond: remote::Bond) {
    remote_link::BOND.lock(|c| c.set(Some(bond)));
    if let Some(mut p) = paired() {
        info!("remote: bond saved");
        p.bond = Some(bond);
        set(fs, Some(p));
    }
}

/// The key being learned skipped (it keeps its old button).
pub(crate) fn skip() {
    LEARNING.lock(|c| {
        if let Some((mut l, _)) = c.get() {
            l.skip();
            c.set(Some((l, None)));
        }
    });
}

/// The learning screen left: what is learned is saved.
pub(crate) fn end_learning(fs: Option<&Fs<'_>>) {
    let Some((l, _)) = LEARNING.lock(Cell::take) else { return };
    if let Some(mut p) = paired() {
        p.buttons = l.buttons();
        info!("remote: {} buttons learned", p.buttons.iter().count());
        set(fs, Some(p));
    }
}

/// A press of the remote: learned on the learning screen, else the key its button does (none: not learned).
pub(crate) fn press(s: remote::Signature, learning: bool) -> Option<ui::Key> {
    if learning {
        LEARNING.lock(|c| {
            let (mut l, _) = c.get().unwrap_or_else(|| (Learner::new(paired().unwrap_or_default().buttons), None));
            let taken = match l.press(s) {
                Learned::Taken(k) => Some(k),
                r => {
                    info!("remote: learning {r:?}");
                    None
                }
            };
            c.set(Some((l, taken)));
        });
        return None;
    }
    let key = paired().and_then(|p| p.buttons.key(&s));
    if key.is_none() {
        info!("remote: button not learned");
    }
    key.and_then(ui::Key::from_nrf)
}

/// The learning screen's content.
pub(crate) fn learn_view() -> ui::Learn {
    let (l, taken) = LEARNING.lock(Cell::get).unwrap_or((Learner::new(Default::default()), None));
    let step = match l.key() {
        Some(k) => remote::LEARN_ORDER.iter().position(|&o| o == k).unwrap_or(0) as u8,
        None => remote::LEARN_ORDER.len() as u8,
    };
    ui::Learn {
        step,
        presses: l.presses(),
        taken: taken.and_then(ui::Key::from_nrf),
        connected: remote_link::connected(),
    }
}

/// The sensors screen's row of the remote.
pub(crate) fn row() -> ui::SensorRow {
    let Some(p) = paired() else { return ui::SensorRow::default() };
    let name = match p.name.as_str() {
        "" => ui::fonts::Line::new(format_args!("BT …{:02X}{:02X}", p.address.addr[1], p.address.addr[0])),
        n => ui::fonts::Line::new(format_args!("{n}")),
    };
    ui::SensorRow {
        link: if remote_link::connected() { ui::Link::Connected } else { ui::Link::Searching },
        name: Some(name),
    }
}
