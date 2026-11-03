//! The remote (crates/remote): a BLE HID device, paired with bonding (the cheap ones ask for it and only know legacy
//! pairing), its input reports turned into button presses ([`PRESSES`]) that the main loop maps to the keys.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::Vec;
use log::{info, warn};
use remote::hid::Layout;
use remote::{Presses, Signature};
use trouble_host::prelude::*;
use trouble_host::{BondInformation, IdentityResolvingKey, LongTermKey};

use super::{Ctrl, HostError, Pool, Server, Target};

/// The presses of the remote's buttons, for the main loop.
pub static PRESSES: Channel<CriticalSectionRawMutex, Signature, 4> = Channel::new();
/// A pairing with the remote left a bond: to save (`/remote`), for the next boots.
pub static BONDED: Signal<CriticalSectionRawMutex, remote::Bond> = Signal::new();
/// The remote's bond to give the stack when the paired devices change ([`super::PAIRED`]); set before.
pub static BOND: BlockingMutex<CriticalSectionRawMutex, Cell<Option<remote::Bond>>> = BlockingMutex::new(Cell::new(None));

static LINKED: AtomicBool = AtomicBool::new(false);

/// The remote is connected, encrypted and its reports followed.
pub fn connected() -> bool {
    LINKED.load(Ordering::Relaxed)
}

const HID: u16 = 0x1812;
const REPORT_MAP: u16 = 0x2A4B;
const REPORT: u16 = 0x2A4D;
const REPORT_REFERENCE: u16 = 0x2908;
/// Report Reference's report type of an input report.
const INPUT: u8 = 1;
/// The link has this long to be encrypted (the remote asks for it at once; a pairing takes ~1 s).
const SECURE_WAIT: Duration = Duration::from_secs(20);
/// Without a pairing started by then (no security request from the remote), we ask.
const ASK_SECURITY_AFTER: Duration = Duration::from_secs(2);
/// Input reports followed at most.
const REPORTS: usize = 8;

/// [`remote::Bond`] for trouble-host.
pub(super) fn bond_information(b: &remote::Bond) -> BondInformation {
    let identity = Identity {
        addr: address(&b.identity),
        irk: b.irk.and_then(IdentityResolvingKey::new),
    };
    let level = match b.level {
        0 => SecurityLevel::NoEncryption,
        1 => SecurityLevel::Encrypted,
        _ => SecurityLevel::EncryptedAuthenticated,
    };
    let mut info = BondInformation::new(identity, LongTermKey::new(b.ltk), level, true);
    info.ediv = b.ediv;
    info.rand = b.rand;
    info.encryption_key_len = b.key_len;
    info
}

fn bond(b: &BondInformation) -> remote::Bond {
    remote::Bond {
        ltk: b.ltk.0,
        ediv: b.ediv,
        rand: b.rand,
        key_len: b.encryption_key_len,
        level: match b.security_level {
            SecurityLevel::NoEncryption => 0,
            SecurityLevel::Encrypted => 1,
            SecurityLevel::EncryptedAuthenticated => 2,
        },
        identity: remote_address(&b.identity.addr),
        irk: b.identity.irk.map(|k| k.0.get()),
    }
}

pub(super) fn address(a: &remote::Address) -> Address {
    Address {
        kind: if a.random { AddrKind::RANDOM } else { AddrKind::PUBLIC },
        addr: BdAddr::new(a.addr),
    }
}

fn remote_address(a: &Address) -> remote::Address {
    let mut addr = [0; 6];
    addr.copy_from_slice(a.addr.raw());
    remote::Address { addr, random: a.kind == AddrKind::RANDOM }
}

/// Encrypts the link (pairing the first time), follows the remote's input reports and sends its presses, until the
/// connection drops.
pub(super) async fn serve(
    stack: &Stack<'_, Ctrl, Pool>,
    server: &Server<'_>,
    conn: &Connection<'_, Pool>,
    t: &Target,
) -> Result<(), HostError> {
    // before the remote's security request is handled: a pairing without bonding would be redone each time
    if let Err(e) = conn.set_bondable(true) {
        warn!("remote: bondable: {e:?}");
    }
    let client = GattClient::<Ctrl, Pool, 8>::new(stack, conn).await?;
    let ours = conn.clone().with_attribute_server(server)?;
    let secure = Signal::<NoopRawMutex, ()>::new();
    let answer = async {
        loop {
            match ours.next().await {
                GattConnectionEvent::Disconnected { .. } => return Ok(()),
                GattConnectionEvent::Gatt { event } => {
                    if let Ok(reply) = event.accept() {
                        reply.send().await;
                    }
                }
                GattConnectionEvent::PairingComplete { security_level, bond: b } => {
                    info!("remote: paired, {security_level:?}, bond {}", b.is_some());
                    if let Some(b) = b {
                        BONDED.signal(bond(&b));
                    }
                    secure.signal(());
                }
                GattConnectionEvent::Encrypted { security_level, .. } => {
                    info!("remote: encrypted, {security_level:?}");
                    secure.signal(());
                }
                GattConnectionEvent::PairingFailed(e) => warn!("remote: pairing failed: {e:?}"),
                GattConnectionEvent::BondLost => warn!("remote: it lost our bond, pairing again"),
                _ => {}
            }
        }
    };
    let watch = async {
        encrypt(conn, &secure).await?;
        let Some(hid) = client.services_by_uuid(&Uuid::new_short(HID)).await?.first().cloned() else {
            warn!("remote: {:?} has no HID service", t.address.addr);
            return Ok(());
        };
        let chars: Vec<Characteristic<[u8]>, 16> = client.characteristics(&hid).await?;
        let mut map = [0u8; 512];
        let mut map_len = 0;
        if let Some(c) = chars.iter().find(|c| c.uuid == Uuid::new_short(REPORT_MAP)) {
            map_len = client.read_characteristic(c, &mut map).await?;
        }
        let layout = Layout::parse(&map[..map_len]);
        info!("remote: report map {map_len} bytes, touch {:?}", layout.touch.is_some());
        // each input report's handle and ID
        let mut reports: Vec<(u16, u8), REPORTS> = Vec::new();
        for c in chars.iter().filter(|c| c.uuid == Uuid::new_short(REPORT) && c.cccd_handle.is_some()) {
            let Ok(r) = client.descriptor_by_uuid::<_, [u8]>(c, &Uuid::new_short(REPORT_REFERENCE)).await else {
                continue;
            };
            let mut reference = [0u8; 2];
            if client.read_handle(r.handle(), &mut reference).await? < 2 || reference[1] != INPUT {
                continue;
            }
            // the listener only to write the CCCD: one catch-all below
            drop(client.subscribe(c, false).await?);
            let _ = reports.push((c.handle, reference[0]));
        }
        info!("remote: following reports {:?}", reports.iter().map(|r| r.1).collect::<Vec<u8, REPORTS>>().as_slice());
        let mut presses = Presses::new(layout);
        let mut listener = client.listen_all()?;
        let _linked = Linked::start();
        loop {
            let now = Instant::now().as_millis();
            let quiet = async {
                match presses.deadline() {
                    Some(d) => Timer::at(Instant::from_millis(d.max(now))).await,
                    None => core::future::pending().await,
                }
            };
            match select(listener.next(), quiet).await {
                Either::First(n) => {
                    if let Some(&(_, id)) = reports.iter().find(|(h, _)| *h == n.handle()) {
                        presses.report(id, n.as_ref(), Instant::now().as_millis());
                    }
                }
                Either::Second(()) => {
                    if let Some(s) = presses.poll(Instant::now().as_millis()) {
                        info!("remote: press {s:?}");
                        if PRESSES.try_send(s).is_err() {
                            warn!("remote: press dropped (main loop busy)");
                        }
                    }
                }
            }
        }
    };
    match select3(client.task(), watch, answer).await {
        Either3::First(r) | Either3::Second(r) | Either3::Third(r) => r,
    }
}

/// Waits for the link to be encrypted: by the bond (asked by the GATT client), or a pairing the remote asks for;
/// asks for one if it does not.
async fn encrypt(conn: &Connection<'_, Pool>, secure: &Signal<NoopRawMutex, ()>) -> Result<(), HostError> {
    let start = Instant::now();
    let mut asked = false;
    loop {
        if conn.security_level()?.encrypted() {
            return Ok(());
        }
        if start.elapsed() >= SECURE_WAIT {
            warn!("remote: not encrypted after {} s, dropping it", SECURE_WAIT.as_secs());
            conn.disconnect();
            return Err(Error::Timeout.into());
        }
        if !asked && start.elapsed() >= ASK_SECURITY_AFTER {
            asked = true;
            info!("remote: asking for security");
            if let Err(e) = conn.request_security() {
                warn!("remote: security request: {e:?}");
            }
        }
        select(secure.wait(), Timer::after(Duration::from_millis(200))).await;
    }
}

/// [`LINKED`] while it lives.
struct Linked;

impl Linked {
    fn start() -> Self {
        LINKED.store(true, Ordering::Relaxed);
        Self
    }
}

impl Drop for Linked {
    fn drop(&mut self) {
        LINKED.store(false, Ordering::Relaxed);
    }
}
