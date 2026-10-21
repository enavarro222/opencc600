//! BLE on the ESP32 (the nRF only does ANT+): the `esp-radio` controller and the `trouble-host` host, central role for
//! the sensors, peripheral role for the companion app (Musette, [`app`]).
//!
//! The paired sensors ([`PAIRED`], from the settings) are connected and kept connected, one connection per device
//! (a device can be several sensors), their measurements decoded by the `sensors` crate into [`current`]. A scan
//! ([`SCAN`]) logs the cycling sensors around, to pair one; it drops the connections while it runs.

use core::cell::{Cell, RefCell};

use embassy_futures::join::join3;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use heapless::Vec;
use log::{info, warn};
use sensors::Revs;
use static_cell::StaticCell;
use companion::Session;
use sensors::ble as gatt;
use settings::Sensor;
use trouble_host::prelude::*;

type Ctrl = ExternalController<BleConnector<'static>, 20>;
type Pool = DefaultPacketPool;
/// Our GATT table: the GAP service (name, appearance; a sensor reading it gets answers, BlueZ waits 30 s for them,
/// then drops the link) and the companion app's service.
type Server<'a> = AttributeServer<'a, NoopRawMutex, Pool, 16, LINKS>;
type HostError = BleHostError<esp_radio::ble::controller::BleConnectorError>;

/// Devices connected at once: a heart rate strap, a speed / cadence sensor, a power meter.
const CONNECTIONS_MAX: usize = 3;
/// The sensors and the companion app.
const LINKS: usize = CONNECTIONS_MAX + 1;
/// Per connection: the signalling and ATT channels.
const L2CAP_CHANNELS_MAX: usize = LINKS * 2;
/// Wheel circumference until it is a setting (700×25C).
const WHEEL_MM: u16 = 2105;
/// A value older than this is gone (the sensor lost, or it stopped sending).
const STALE: Duration = Duration::from_secs(5);

/// The paired sensors, from the settings: sent at boot and when they change.
pub static PAIRED: Signal<CriticalSectionRawMutex, Paired> = Signal::new();
/// A scan for this many seconds, the sensors found in [`found`] and the log; sent again during a scan, it moves its
/// end (0: stop now).
pub static SCAN: Signal<CriticalSectionRawMutex, u16> = Signal::new();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Paired {
    pub hr: Sensor,
    pub speed_cadence: Sensor,
    pub power: Sensor,
}

/// The sensor values now (`None`: no sensor, or nothing received for [`STALE`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Current {
    pub heart_rate: Option<u8>,
    pub cadence_rpm: Option<u8>,
    /// From the wheel sensor.
    pub speed_mm_s: Option<u32>,
    pub power_w: Option<u16>,
}

pub fn current() -> Current {
    let r = READINGS.lock(Cell::get);
    let now = Instant::now();
    fn fresh<T>(v: Option<(T, Instant)>, now: Instant) -> Option<T> {
        v.filter(|(_, at)| now.saturating_duration_since(*at) < STALE).map(|(v, _)| v)
    }
    Current {
        heart_rate: fresh(r.hr, now),
        // the speed / cadence sensor's, else the power meter's
        cadence_rpm: fresh(r.cadence, now)
            .or(fresh(r.power_cadence, now))
            .map(|c| c.min(254) as u8),
        speed_mm_s: fresh(r.speed_mm_s, now),
        power_w: fresh(r.power, now),
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Readings {
    hr: Option<(u8, Instant)>,
    cadence: Option<(u16, Instant)>,
    power_cadence: Option<(u16, Instant)>,
    speed_mm_s: Option<(u32, Instant)>,
    power: Option<(u16, Instant)>,
}

static READINGS: BlockingMutex<CriticalSectionRawMutex, Cell<Readings>> = BlockingMutex::new(Cell::new(Readings {
    hr: None,
    cadence: None,
    power_cadence: None,
    speed_mm_s: None,
    power: None,
}));

fn reading(f: impl FnOnce(&mut Readings)) {
    READINGS.lock(|c| {
        let mut r = c.get();
        f(&mut r);
        c.set(r);
    });
}

/// A device to connect, and what it is paired for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    address: Address,
    hr: bool,
    speed_cadence: bool,
    power: bool,
}

/// The BLE devices of the paired sensors, each once.
fn targets(p: &Paired) -> Vec<Target, CONNECTIONS_MAX> {
    let mut out: Vec<Target, CONNECTIONS_MAX> = Vec::new();
    for (i, s) in [p.hr, p.speed_cadence, p.power].into_iter().enumerate() {
        let Sensor::Ble { random, addr } = s else {
            continue;
        };
        let address = Address {
            kind: if random { AddrKind::RANDOM } else { AddrKind::PUBLIC },
            addr: BdAddr::new(addr),
        };
        let t = match out.iter_mut().find(|t| t.address == address) {
            Some(t) => t,
            None => {
                let _ = out.push(Target { address, hr: false, speed_cadence: false, power: false });
                out.last_mut().unwrap()
            }
        };
        match i {
            0 => t.hr = true,
            1 => t.speed_cadence = true,
            _ => t.power = true,
        }
    }
    out
}

/// Runs the BLE stack: waits for the paired sensors, connects them, scans on request; waits for the companion app,
/// which asks `device`.
pub async fn run(bt: BT<'static>, mut device: impl companion::Device) {
    let connector = match BleConnector::new(bt, Default::default()) {
        Ok(c) => c,
        Err(e) => {
            warn!("ble: controller init failed: {e:?}");
            return;
        }
    };
    info!("ble: controller up, heap {}", esp_alloc::HEAP.stats());
    let controller: Ctrl = ExternalController::new(connector);
    // a static random address (top two bits set), fixed for now
    let address = Address::random([0x6f, 0xcc, 0x60, 0x00, 0x5c, 0xc6]);
    // ~18 KB: static, not in the task's future
    static RESOURCES: StaticCell<HostResources<Ctrl, Pool, LINKS, L2CAP_CHANNELS_MAX>> = StaticCell::new();
    let stack = trouble_host::new(controller, RESOURCES.init(HostResources::new()))
        .set_random_address(address)
        .build();
    let mut runner = stack.runner();
    let central = Mutex::<NoopRawMutex, _>::new(Some(stack.central()));
    let mut table = AttributeTable::new();
    let gap = GapConfig::Peripheral(PeripheralConfig {
        name: NAME,
        appearance: &appearance::cycling::CYCLING_COMPUTER,
    });
    if let Err(e) = gap.build(&mut table) {
        warn!("ble: GAP service: {e}");
    }
    let (rx, tx) = app_service(&mut table);
    let server: Server<'_> = AttributeServer::new(table);
    let seen = Seen::default();
    let host = async {
        if let Err(e) = runner.run_with_handler(&seen).await {
            warn!("ble: host stopped: {e:?}");
        }
    };
    let _ = join3(host, app(&stack, &server, &rx, &tx, &mut device), async {
        let mut paired = Paired::default();
        loop {
            let t = targets(&paired);
            let slots = join3(
                slot(&stack, &server, &central, t.first()),
                slot(&stack, &server, &central, t.get(1)),
                slot(&stack, &server, &central, t.get(2)),
            );
            match select3(PAIRED.wait(), SCAN.wait(), slots).await {
                Either3::First(p) => {
                    info!("ble: paired {p:?}");
                    paired = p;
                }
                Either3::Second(0) => {}
                Either3::Second(seconds) => scan(&central, &seen, seconds).await,
                Either3::Third(_) => {}
            }
        }
    })
    .await;
}

/// Keeps one device connected; never returns (pending without a device).
async fn slot(
    stack: &Stack<'_, Ctrl, Pool>,
    server: &Server<'_>,
    central: &Mutex<NoopRawMutex, Option<Central<'_, Ctrl, Pool>>>, target: Option<&Target>) {
    let Some(t) = target else {
        return core::future::pending().await;
    };
    // since the device was last connected (or this slot started): the attempts thin out after a minute
    let mut seen = Instant::now();
    loop {
        let conn = {
            let mut c = central.lock().await;
            let Some(c) = c.as_mut() else {
                return core::future::pending().await;
            };
            let config = ConnectConfig {
                scan_config: ScanConfig {
                    filter_accept_list: &[t.address],
                    ..Default::default()
                },
                connect_params: Default::default(),
            };
            // the radio is shared by the slots: give it back now and then while the device is away
            let _listening = Listening::start();
            with_timeout(LISTEN, c.connect(&config)).await
        };
        match conn {
            Ok(Ok(conn)) => {
                info!("ble: connected {:?}", t.address.addr);
                let sensor = sensor_of(&t.address);
                LINKED.lock(|l| {
                    let mut l = l.borrow_mut();
                    l.retain(|(s, _)| *s != sensor);
                    let _ = l.push((sensor, Name::new()));
                });
                if let Err(e) = serve(stack, server, &conn, t).await {
                    warn!("ble: {:?}: {e:?}", t.address.addr);
                }
                LINKED.lock(|l| l.borrow_mut().retain(|(s, _)| *s != sensor));
                info!("ble: disconnected {:?}", t.address.addr);
                seen = Instant::now();
            }
            Ok(Err(e)) => warn!("ble: connect {:?}: {e:?}", t.address.addr),
            Err(_) => {}
        }
        Timer::after(if seen.elapsed() < SEEK_FAST { Duration::from_secs(1) } else { SEEK_PAUSE }).await;
    }
}

/// A connection attempt listens this long for the device.
const LISTEN: Duration = Duration::from_secs(5);
/// A device away (asleep, out of range, left at home): looked for at once for a minute after it was last there,
/// then [`LISTEN`] every [`SEEK_PAUSE`] — the radio listening all the time costs battery and may disturb the GNSS
/// next to it (to measure).
const SEEK_FAST: Duration = Duration::from_secs(60);
const SEEK_PAUSE: Duration = Duration::from_secs(25);

/// Connection attempts and scans running now, for the trip log ([`listening`]).
static LISTENING: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The radio is listening for devices now (a connection attempt, a scan).
pub fn listening() -> bool {
    LISTENING.load(core::sync::atomic::Ordering::Relaxed) > 0
}

/// The devices connected now.
pub fn links() -> usize {
    LINKED.lock(|l| l.borrow().len())
}

/// Counts in [`LISTENING`] while it lives.
struct Listening;

impl Listening {
    fn start() -> Self {
        LISTENING.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        LISTENING.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// What a notification handle carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    HeartRate,
    SpeedCadence,
    Power,
}

/// Subscribes to the measurements the device is paired for and decodes them until the connection drops.
async fn serve(
    stack: &Stack<'_, Ctrl, Pool>,
    server: &Server<'_>,
    conn: &Connection<'_, Pool>,
    t: &Target,
) -> Result<(), HostError> {
    let client = GattClient::<Ctrl, Pool, 16>::new(stack, conn).await?;
    let sensor = sensor_of(&t.address);
    let ours = conn.clone().with_attribute_server(server)?;
    // the peer's requests to our table
    let answer = async {
        loop {
            match ours.next().await {
                GattConnectionEvent::Disconnected { .. } => return Ok(()),
                GattConnectionEvent::Gatt { event } => {
                    if let Ok(reply) = event.accept() {
                        reply.send().await;
                    }
                }
                _ => {}
            }
        }
    };
    let watch = async {
        // its name (GAP), for the sensors screen
        if let Some(gap) = client.services_by_uuid(&Uuid::new_short(0x1800)).await?.first() {
            let mut buf = [0; 18];
            if let Ok(n) = client.read_characteristic_by_uuid(gap, &Uuid::new_short(0x2A00), &mut buf).await {
                let n = name(&buf[..n]);
                info!("ble: {:?} is {:?}", t.address.addr, n.as_str());
                LINKED.lock(|l| {
                    if let Some(e) = l.borrow_mut().iter_mut().find(|(s, _)| *s == sensor) {
                        e.1 = n;
                    }
                });
            }
        }
        let mut handles: Vec<(u16, Kind), 3> = Vec::new();
        for (wanted, service, measurement, kind) in [
            (t.hr, gatt::HEART_RATE, gatt::HEART_RATE_MEASUREMENT, Kind::HeartRate),
            (t.speed_cadence, gatt::SPEED_CADENCE, gatt::CSC_MEASUREMENT, Kind::SpeedCadence),
            (t.power, gatt::POWER, gatt::POWER_MEASUREMENT, Kind::Power),
        ] {
            if !wanted {
                continue;
            }
            let services = client.services_by_uuid(&Uuid::new_short(service)).await?;
            let Some(s) = services.first() else {
                warn!("ble: {:?} has no {service:04x} service", t.address.addr);
                for s in client.services().await? {
                    info!("ble:   service {:?}", s.uuid());
                }
                continue;
            };
            let c: Characteristic<[u8; 1]> = client.characteristic_by_uuid(s, &Uuid::new_short(measurement)).await?;
            // the listener only to write the CCCD: one catch-all below (one subscriber at most)
            drop(client.subscribe(&c, false).await?);
            let _ = handles.push((c.handle, kind));
            info!("ble: subscribed {kind:?}");
        }
        let mut listener = client.listen_all()?;
        let (mut crank, mut wheel, mut power_crank) = (Revs::default(), Revs::default(), Revs::default());
        let mut logged = Instant::now();
        loop {
            let n = listener.next().await;
            if logged.elapsed() >= Duration::from_secs(10) {
                logged = Instant::now();
                info!("ble: {:?}", current());
            }
            let Some(&(_, kind)) = handles.iter().find(|(h, _)| *h == n.handle()) else {
                continue;
            };
            let data = n.as_ref();
            let now = Instant::now();
            let ms = now.as_millis();
            match kind {
                Kind::HeartRate => {
                    if let Some(bpm) = gatt::heart_rate(data) {
                        reading(|r| r.hr = Some((bpm, now)));
                    }
                }
                Kind::SpeedCadence => {
                    let Some(m) = gatt::csc(data) else { continue };
                    if let Some(c) = m.crank {
                        crank.update(c.revs, c.time, u16::MAX as u32, ms);
                        if let Some(rpm) = crank.rpm() {
                            reading(|r| r.cadence = Some((rpm, now)));
                        }
                    }
                    if let Some(w) = m.wheel {
                        wheel.update(w.revs, w.time, u32::MAX, ms);
                        if let Some(v) = wheel.speed_mm_s(WHEEL_MM) {
                            reading(|r| r.speed_mm_s = Some((v, now)));
                        }
                    }
                }
                Kind::Power => {
                    let Some(m) = gatt::power(data) else { continue };
                    if let Some(c) = m.crank {
                        power_crank.update(c.revs, c.time, u16::MAX as u32, ms);
                        if let Some(rpm) = power_crank.rpm() {
                            reading(|r| r.power_cadence = Some((rpm, now)));
                        }
                    }
                    reading(|r| r.power = Some((m.watts, now)));
                }
            }
        }
    };
    match select3(client.task(), watch, answer).await {
        Either3::First(r) | Either3::Second(r) | Either3::Third(r) => r,
    }
}

/// Scans until [`SCAN`] says 0 or for the last number of seconds it gave, filling [`found`].
async fn scan(central: &Mutex<NoopRawMutex, Option<Central<'_, Ctrl, Pool>>>, seen: &Seen, seconds: u16) {
    let mut c = central.lock().await;
    let Some(inner) = c.take() else { return };
    let mut scanner = Scanner::new(inner);
    let config = ScanConfig {
        active: true,
        phys: PhySet::M1,
        interval: Duration::from_millis(100),
        window: Duration::from_millis(50),
        ..Default::default()
    };
    seen.start();
    let _listening = Listening::start();
    // right after a connection attempt was cancelled the controller can still refuse ("Command Disallowed"): retry
    for attempt in 0..10 {
        match scanner.scan(&config).await {
            Ok(_session) => {
                info!("ble: scanning {seconds} s");
                let mut seconds = seconds;
                // a new request moves the end (0: now)
                while seconds > 0 {
                    match select(Timer::after(Duration::from_secs(seconds.into())), SCAN.wait()).await {
                        Either::First(()) => seconds = 0,
                        Either::Second(s) => seconds = s,
                    }
                }
                break;
            }
            Err(e) if attempt < 9 => {
                info!("ble: scan refused ({e:?}), again");
                Timer::after(Duration::from_millis(200)).await;
            }
            Err(e) => warn!("ble: scan failed: {e:?}"),
        }
    }
    seen.stop();
    info!("ble: scan done");
    *c = Some(scanner.into_inner());
}

/// A device heard by the last scan: what its advertisements said, merged (the name often comes in the scan
/// response, without the services).
#[derive(Clone, Debug)]
pub struct Heard {
    pub sensor: Sensor,
    pub name: Name,
    pub rssi_dbm: i8,
    /// [`ROLE_HR`], [`ROLE_SPEED_CADENCE`], [`ROLE_POWER`] bits: the services it advertises.
    pub roles: u8,
}

/// A device name, printable ASCII (the screen fonts have it), cut to fit a row.
pub type Name = heapless::String<18>;

pub const ROLE_HR: u8 = 1;
pub const ROLE_SPEED_CADENCE: u8 = 2;
pub const ROLE_POWER: u8 = 4;

/// Devices kept from a scan; the ones without a sensor service go first when it is full.
const HEARD_MAX: usize = 24;

static HEARD: BlockingMutex<CriticalSectionRawMutex, RefCell<Vec<Heard, HEARD_MAX>>> =
    BlockingMutex::new(RefCell::new(Vec::new()));

/// The sensors the last scan heard for a role (`ROLE_*` bit), closest first.
pub fn found<const N: usize>(role: u8) -> Vec<Heard, N> {
    HEARD.lock(|h| {
        let mut all: Vec<Heard, HEARD_MAX> = h.borrow().iter().filter(|d| d.roles & role != 0).cloned().collect();
        all.sort_unstable_by_key(|d| core::cmp::Reverse(d.rssi_dbm));
        all.into_iter().take(N).collect()
    })
}

/// The name a device gave (scan or GAP), if heard.
pub fn name_of(sensor: &Sensor) -> Option<Name> {
    let from_scan = HEARD.lock(|h| h.borrow().iter().find(|d| d.sensor == *sensor && !d.name.is_empty()).map(|d| d.name.clone()));
    from_scan.or_else(|| LINKED.lock(|l| l.borrow().iter().find(|(s, _)| s == sensor).map(|(_, n)| n.clone())))
}

/// The devices connected now, with the name they gave (GAP).
static LINKED: BlockingMutex<CriticalSectionRawMutex, RefCell<Vec<(Sensor, Name), CONNECTIONS_MAX>>> =
    BlockingMutex::new(RefCell::new(Vec::new()));

pub fn connected(sensor: &Sensor) -> bool {
    LINKED.lock(|l| l.borrow().iter().any(|(s, _)| s == sensor))
}

fn name(raw: &[u8]) -> Name {
    let mut n = Name::new();
    for &b in raw {
        let c = if (0x20..0x7F).contains(&b) { b as char } else { '?' };
        if n.push(c).is_err() {
            break;
        }
    }
    let trimmed = n.trim_end().len();
    n.truncate(trimmed);
    n
}

fn sensor_of(address: &Address) -> Sensor {
    let mut addr = [0; 6];
    addr.copy_from_slice(address.addr.raw());
    Sensor::Ble { random: address.kind == AddrKind::RANDOM, addr }
}

/// The scan's reports, into [`HEARD`] while a scan runs.
#[derive(Default)]
struct Seen {
    on: Cell<bool>,
}

impl Seen {
    fn start(&self) {
        HEARD.lock(|h| h.borrow_mut().clear());
        self.on.set(true);
    }
    fn stop(&self) {
        self.on.set(false);
    }
}

impl EventHandler for Seen {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        if !self.on.get() {
            return;
        }
        while let Some(Ok(report)) = it.next() {
            let Ok(addr) = <[u8; 6]>::try_from(report.addr.raw()) else {
                continue;
            };
            let sensor = Sensor::Ble { random: report.addr_kind == AddrKind::RANDOM, addr };
            let mut heard_name: &[u8] = &[];
            let mut roles = 0;
            for ad in AdStructure::decode(report.data).flatten() {
                match ad {
                    AdStructure::CompleteLocalName(n) | AdStructure::ShortenedLocalName(n) => heard_name = n,
                    AdStructure::CompleteServiceUuids16(u) | AdStructure::IncompleteServiceUuids16(u) => {
                        for u in u {
                            roles |= match u16::from_le_bytes(*u) {
                                gatt::HEART_RATE => ROLE_HR,
                                gatt::SPEED_CADENCE => ROLE_SPEED_CADENCE,
                                gatt::POWER => ROLE_POWER,
                                _ => 0,
                            };
                        }
                    }
                    _ => {}
                }
            }
            HEARD.lock(|h| {
                let mut h = h.borrow_mut();
                let i = match h.iter().position(|d| d.sensor == sensor) {
                    Some(i) => i,
                    None => {
                        if h.is_full() {
                            // make room: a device without a sensor service, else drop this one
                            let Some(j) = h.iter().position(|d| d.roles == 0) else { return };
                            h.swap_remove(j);
                        }
                        let _ = h.push(Heard { sensor, name: Name::new(), rssi_dbm: report.rssi, roles: 0 });
                        h.len() - 1
                    }
                };
                let d = &mut h[i];
                d.rssi_dbm = report.rssi;
                if !heard_name.is_empty() {
                    d.name = name(heard_name);
                }
                if roles & !d.roles != 0 {
                    d.roles |= roles;
                    let kinds: Vec<&str, 3> = [(ROLE_HR, "hr"), (ROLE_SPEED_CADENCE, "speed_cadence"), (ROLE_POWER, "power")]
                        .into_iter()
                        .filter(|(r, _)| d.roles & r != 0)
                        .map(|(_, k)| k)
                        .collect();
                    info!("ble: found {} {:?} rssi {} dBm: {:?}", d.sensor, d.name.as_str(), d.rssi_dbm, kinds.as_slice());
                }
            });
        }
    }
}

/// Our name: the app looks for devices whose name starts with `protocol_cc600::ble::NAME_PREFIX`.
const NAME: &str = "CC600 OpenCC600";
/// Largest write of the app (an ATT MTU of 517, as Chrome asks) and largest notification.
const ATT_VALUE_MAX: usize = 512;
/// Advertising interval while waiting for the app: slow, the radio is shared with the sensors (and may disturb the
/// GNSS); the app finds the device within a few seconds.
const ADVERTISE: Duration = Duration::from_millis(1000);
/// Advertising is started again after this long without the app. A connection that fails before it is accepted
/// (`Connection Failed to be Established`, seen with a busy main loop) never comes out of `accept` (trouble-host 0.7),
/// and the controller stopped advertising when it was made: the device was invisible until a reset.
const ADVERTISE_AGAIN: Duration = Duration::from_secs(10);

/// What the companion app did: a file arriving or received, settings changed (for the screen).
pub static APP: Signal<CriticalSectionRawMutex, companion::Event> = Signal::new();
/// The companion app is connected.
static APP_LINKED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn app_linked() -> bool {
    APP_LINKED.load(core::sync::atomic::Ordering::Relaxed)
}

type Value = heapless::Vec<u8, ATT_VALUE_MAX>;

/// The companion app's service (`protocol-cc600`): the app writes frames on RX, the answers are notified on TX.
fn app_service(
    table: &mut AttributeTable<'static, NoopRawMutex, 16>,
) -> (Characteristic<Value>, Characteristic<Value>) {
    static STORES: StaticCell<[[u8; ATT_VALUE_MAX]; 2]> = StaticCell::new();
    let [rx_store, tx_store] = STORES.init([[0; ATT_VALUE_MAX]; 2]);
    let mut service = table.add_service(Service::new(Uuid::new_long(SERVICE)));
    let rx = service
        .add_characteristic(
            Uuid::new_long(RX),
            [CharacteristicProp::Write, CharacteristicProp::WriteWithoutResponse],
            Value::new(),
            rx_store,
        )
        .build();
    let tx = service
        .add_characteristic(Uuid::new_long(TX), [CharacteristicProp::Notify], Value::new(), tx_store)
        .build();
    service.build();
    (rx, tx)
}

/// Advertises, serves the app while it is connected, again; never returns. The session outlives the connections: a
/// file resumes after a dropped link.
async fn app(
    stack: &Stack<'_, Ctrl, Pool>,
    server: &Server<'_>,
    rx: &Characteristic<Value>,
    tx: &Characteristic<Value>,
    device: &mut impl companion::Device,
) {
    let mut session = Session::new();
    let mut peripheral = stack.peripheral();
    let mut adv = [0; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[SERVICE]),
            AdStructure::ShortenedLocalName(protocol_cc600::ble::NAME_PREFIX.as_bytes()),
        ],
        &mut adv,
    )
    .unwrap_or(0);
    let mut scan_data = [0; 31];
    let scan_len =
        AdStructure::encode_slice(&[AdStructure::CompleteLocalName(NAME.as_bytes())], &mut scan_data).unwrap_or(0);
    let params = AdvertisementParameters { interval_min: ADVERTISE, interval_max: ADVERTISE, ..Default::default() };
    loop {
        let advertised = peripheral
            .advertise(
                &params,
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv[..len],
                    scan_data: &scan_data[..scan_len],
                },
            )
            .await;
        let conn = match advertised {
            Ok(advertiser) => match with_timeout(ADVERTISE_AGAIN, advertiser.accept()).await {
                Ok(conn) => conn,
                Err(_) => continue,
            },
            Err(e) => {
                warn!("ble: advertising: {e:?}");
                Timer::after(Duration::from_secs(5)).await;
                continue;
            }
        };
        let conn = match conn.map(|c| c.with_attribute_server(server)) {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                warn!("ble: app: {e:?}");
                continue;
            }
            Err(e) => {
                warn!("ble: app connection: {e:?}");
                continue;
            }
        };
        info!("ble: app connected");
        APP_LINKED.store(true, core::sync::atomic::Ordering::Relaxed);
        serve_app(&conn, rx, tx, device, &mut session).await;
        APP_LINKED.store(false, core::sync::atomic::Ordering::Relaxed);
        session.disconnected();
        info!("ble: app disconnected");
    }
}

/// Feeds the app's writes to the session and notifies its answers, until the app leaves.
async fn serve_app(
    conn: &GattConnection<'_, '_, Pool>,
    rx: &Characteristic<Value>,
    tx: &Characteristic<Value>,
    device: &mut impl companion::Device,
    session: &mut Session,
) {
    loop {
        let GattConnectionEvent::Gatt { event } = (match conn.next().await {
            GattConnectionEvent::Disconnected { .. } => return,
            other => other,
        }) else {
            continue;
        };
        let written = match &event {
            GattEvent::Write(w) if w.handle() == rx.handle => Some(w.with_data(|_, data| data.to_vec())),
            _ => None,
        };
        if let Ok(reply) = event.accept() {
            reply.send().await;
        }
        // after the write response: a ride's piece waits for the main loop, which owns the storage
        let events = match written {
            Some(data) => session.on_write(&data, device).await,
            None => alloc::vec::Vec::new(),
        };
        for e in events {
            match e {
                companion::Event::Receiving { kind, received, size } => info!("ble: app {kind:?} {received}/{size}"),
                companion::Event::Received(kind) => info!("ble: app {kind:?} received"),
                companion::Event::SettingsChanged => info!("ble: app changed settings"),
            }
            APP.signal(e);
        }
        let out = session.take_output();
        // a notification carries the ATT MTU less its 3-byte header; the app reassembles the frames
        let piece = (conn.raw().att_mtu() as usize).saturating_sub(3).clamp(20, ATT_VALUE_MAX);
        for p in out.chunks(piece) {
            if let Err(e) = tx.notify_raw(conn, p, false).await {
                warn!("ble: app notify: {e:?}");
                break;
            }
        }
    }
}

/// The UUIDs of `protocol-cc600`, as the 16 bytes on the air.
const SERVICE: [u8; 16] = uuid(protocol_cc600::ble::SERVICE_UUID);
const RX: [u8; 16] = uuid(protocol_cc600::ble::RX_CHAR_UUID);
const TX: [u8; 16] = uuid(protocol_cc600::ble::TX_CHAR_UUID);

/// `18fbbd9a-adda-...` to bytes, least significant first.
const fn uuid(text: &str) -> [u8; 16] {
    const fn hex(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("not a UUID"),
        }
    }
    let b = text.as_bytes();
    let mut out = [0; 16];
    let (mut i, mut n) = (0, 0);
    while i < b.len() {
        if b[i] == b'-' {
            i += 1;
            continue;
        }
        out[15 - n] = hex(b[i]) * 16 + hex(b[i + 1]);
        n += 1;
        i += 2;
    }
    assert!(n == 16, "not a UUID");
    out
}
