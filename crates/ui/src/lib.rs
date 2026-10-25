//! Ride UI of the CC600, hardware independent: the screens, what the 3 keys do on each, and their drawing on the
//! 240x320 panel ([`draw`]). The design follows the clickable prototype (docs/prototype), in French.
//!
//! The firmware owns the ride: [`Ui::key`] only returns a [`Command`] for it to run, and [`Ui::tick`] brings the
//! screen back in line with the ride it really has (a start refused without the time, a ride lost).
//!
//! Keys (nRF numbering): 0 top left, 1 bottom left, 2 bottom right. Bottom right is the main action (start, pause,
//! resume, OK), bottom left the menu or the next page / item, top left back or contextual. One long press only:
//! the top left key on the home screen switches off. A ride ends from the pause screen, confirmed with another
//! key, and stays resumable afterwards. A ride started by mistake is erased: from the same key when it is empty
//! ([`RideView::empty`]), else from the pause menu with two confirmations.

#![no_std]

mod draw;
pub mod light;
pub mod fonts;
pub mod history;
mod map;
pub mod nav;

pub use draw::{CONTENT, DAY, NIGHT, Theme, draw};
use fonts::Line;
pub use settings::Zones;

/// Pages of a ride at most.
pub const MAX_PAGES: usize = 4;
/// Auto-scroll of the ride pages, one page every this long (default of [`Ui::set_scroll_s`]).
pub const SCROLL_MS: u64 = 4_000;
/// An information banner stays this long.
pub const INFO_MS: u64 = 3_000;

/// A progress goes this long after its last news (a transfer cut short).
pub const PROGRESS_MS: u64 = 5_000;
/// A power or heart rate zone shows once the value has stayed this long in it: at a bound, the screen does not
/// flicker from one colour to the other.
pub const ZONE_HOLD_MS: u64 = 3_000;
/// Points of an altitude profile ([`View::recent`], [`View::whole`]).
pub const PROFILE: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    TopLeft,
    BottomLeft,
    BottomRight,
}

impl Key {
    /// Key number of the nRF key frames.
    pub fn from_nrf(key: u8) -> Option<Self> {
        match key {
            0 => Some(Self::TopLeft),
            1 => Some(Self::BottomLeft),
            2 => Some(Self::BottomRight),
            _ => None,
        }
    }
}

/// The nRF repeats `Long` while the key is held (every ~200 ms): a hold acts once ([`Ui::key`] drops the repeats),
/// so a long press does nothing more on the screen it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Click,
    Long,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Menu,
    /// Bring-up values (sensors, links, storage), drawn by the firmware in [`CONTENT`].
    Status,
    /// At power-on, until the firmware knows what to show (the nRF's sensor stream there or not): the logo.
    Splash,
    /// Started by the charger (switched off and plugged in, like the vendor's "off, charging"), or without the nRF's
    /// sensor stream: the battery charging; a long press of the power key switches on.
    Charging,
    /// The sensor roles (heart rate, speed / cadence, power) and their sensor.
    Sensors,
    /// The sensors found around for a role ([`Ui::role`]), to pick one ([`View::sensors`]).
    SensorScan,
    /// The power meter's zero offset calibration: what to do, then its result ([`Sensors::calibration`]).
    Calibrate,
    Ride,
    Paused,
    PauseMenu,
    /// A paused ride found at power-on.
    Resume,
    ConfirmStop,
    /// Finishing an empty ride ([`RideView::empty`]): erasing it is the main key, nothing is lost.
    ConfirmEmpty,
    /// "Abandon the ride?", from the pause menu, then [`Screen::ConfirmDiscardAgain`].
    ConfirmDiscard,
    /// The last confirmation, the keys the other way round: erasing on the bottom left, the main key keeps the ride
    /// (two presses out of habit erase nothing).
    ConfirmDiscardAgain,
    Saved,
    /// The finished rides, one at a time ([`View::rides`]), to delete some: from the menu, or when a ride cannot start
    /// for want of room ([`Ui::make_room`]).
    Rides,
    /// "Delete this ride?"
    ConfirmDelete,
    /// The version and a QR code to the project's page ([`ABOUT_URL`]).
    About,
}

/// A ride page, picked when the ride starts ([`pages`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The route around the rider, heading up.
    Map,
    /// Speed, time, distance, D+, heart rate (else average speed).
    Essential,
    /// Altitude, gradient, D+, climbing speed, the last 5 km.
    Altitude,
    /// The climbs of the route; in one, what is left of it (a route with climbs, in place of Altitude).
    Climbs,
    /// 3 s power, then average power, heart rate, cadence, speed, time (a power meter paired).
    Effort,
    /// Heart rate, then its average and max, speed, cadence or time (a heart rate sensor without a power meter).
    Cardio,
}

/// The effort page of a ride: with a power meter [`Page::Effort`], else with a heart rate sensor [`Page::Cardio`]
/// ([`EffortPick`]). In order: a power meter beats a heart rate sensor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effort {
    #[default]
    None,
    Heart,
    Power,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Map => "Carte",
            Page::Essential => "Essentiel",
            Page::Altitude => "Altitude",
            Page::Climbs => "Montées",
            Page::Effort => "Effort",
            Page::Cardio => "Cardio",
        }
    }
}

/// The ride pages for what the device has (docs/PRODUCT.md, "Data pages"): to explore, the map first when a route
/// is loaded, then Essential and Altitude, or Climbs in its place when the route has climbs (`climbs`); Effort
/// (or Cardio) last with a power meter (or a heart rate sensor) paired (`effort`); to train, Effort first.
pub fn pages(route: bool, climbs: bool, train: bool, effort: crate::Effort) -> Pages {
    use Page::*;
    let effort = match effort {
        crate::Effort::None => None,
        crate::Effort::Heart => Some(Cardio),
        crate::Effort::Power => Some(Effort),
    };
    let height = Some(if route && climbs { Climbs } else { Altitude });
    let list = match (route, train) {
        (true, false) => [Some(Map), Some(Essential), height, effort],
        (true, true) => [effort, Some(Essential), Some(Map), height],
        (false, false) => [Some(Essential), height, effort, None],
        (false, true) => [effort, Some(Essential), height, None],
    };
    // the missing Effort takes no slot
    let mut list = list.into_iter().flatten();
    let list = core::array::from_fn(|_| list.next());
    Pages { list }
}

/// After the start of a ride (or a sensor paired or forgotten), a sensor that connects still brings its page.
pub const EFFORT_GRACE_MS: u64 = 5 * 60_000;

/// Picks the effort page from the sensors connected, not from the ones paired: a power meter off at the start
/// leaves Cardio to the heart rate sensor. Within [`EFFORT_GRACE_MS`] of the start, a sensor that connects brings
/// its page (Cardio becomes Effort); later, a sensor lost keeps its page (its values "–"). A sensor forgotten takes
/// its page away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffortPick {
    picked: Effort,
    /// Since when the pages may still change, during a ride.
    since_ms: Option<u64>,
    /// The roles paired, at the last update.
    paired: [bool; 3],
}

impl EffortPick {
    /// The effort page now: `ride` a ride is in progress (running or paused).
    pub fn update(&mut self, ride: bool, sensors: &Sensors, now_ms: u64) -> Effort {
        let link = |role: Role| sensors.roles[Role::ALL.iter().position(|&r| r == role).unwrap_or(0)].link;
        let has = |role: Role, l: Link| link(role) == l;
        let seen = if has(Role::Power, Link::Connected) {
            Effort::Power
        } else if has(Role::HeartRate, Link::Connected) {
            Effort::Heart
        } else {
            Effort::None
        };
        let paired = sensors.roles.map(|r| r.link != Link::Unpaired);
        if !ride {
            *self = Self { picked: seen, since_ms: None, paired };
            return seen;
        }
        let since = self.since_ms.get_or_insert(now_ms);
        // a sensor paired or forgotten during the ride: its page may come
        if paired != self.paired {
            *since = now_ms;
            self.paired = paired;
        }
        if now_ms.saturating_sub(*since) < EFFORT_GRACE_MS {
            self.picked = self.picked.max(seen);
        }
        // forgotten
        if self.picked == Effort::Power && has(Role::Power, Link::Unpaired) {
            self.picked = if has(Role::HeartRate, Link::Connected) { Effort::Heart } else { Effort::None };
        }
        if self.picked == Effort::Heart && has(Role::HeartRate, Link::Unpaired) {
            self.picked = Effort::None;
        }
        self.picked
    }
}

/// Map zooms around the rider: metres for the scale bar (60 pixels). The top left key goes through them, then the
/// whole route.
pub const ZOOMS_M: [u32; 4] = [250, 500, 1000, 2000];
/// Zoom steps of the map: [`ZOOMS_M`], then the whole route.
pub const ZOOM_STEPS: u8 = ZOOMS_M.len() as u8 + 1;
/// Off the route farther than this, decimetres: the banner. Back on it under half of it.
pub const OFF_ROUTE_DM: u32 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pages {
    list: [Option<Page>; MAX_PAGES],
}

impl Pages {
    pub fn len(&self) -> usize {
        self.list.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<Page> {
        self.list.get(i).copied().flatten()
    }

    /// "Essentiel · Altitude".
    pub fn names(&self) -> Line {
        let mut l = Line::new(format_args!(""));
        for (i, p) in self.list.iter().flatten().enumerate() {
            let sep = if i > 0 { " · " } else { "" };
            l = Line::new(format_args!("{}{sep}{}", l.as_str(), p.name()));
        }
        l
    }
}

/// What the firmware has to do after a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    Pause,
    Resume,
    /// Finish the ride: it stays resumable.
    Stop,
    /// Erase the ride in progress, not saved.
    Discard,
    /// Resume the last finished ride (reopened, timer running).
    ResumeLast,
    /// Switch off; a paused ride is kept and offered again at power-on.
    PowerOff,
    /// Look for the sensors of a role around, until [`Command::StopScan`]: found ones go in [`Sensors::found`].
    Scan(Role),
    StopScan,
    /// Pair the role with this entry of [`Sensors::found`].
    Pair(Role, u8),
    /// No sensor for the role any more.
    Forget(Role),
    /// Calibrate the power meter (zero offset).
    Calibrate,
    /// Follow the loaded route (`true`), or set it aside for a free ride: it stays stored.
    FollowRoute(bool),
    /// Switched on from the charging screen (the long press starts the nRF's sensor stream, ~400 ms later): the logo
    /// until it comes, as at power-on.
    WakeUp,
    /// Delete this entry of the rides screen ([`Ui::ride_item`]).
    DeleteRide(u16),
}

/// What a sensor is paired for, in the order of the sensors screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    HeartRate,
    SpeedCadence,
    Power,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::HeartRate, Role::SpeedCadence, Role::Power];

    pub fn label(self) -> &'static str {
        match self {
            Role::HeartRate => "Cardio",
            Role::SpeedCadence => "Vitesse / cadence",
            Role::Power => "Puissance",
        }
    }

    /// Upper case, for the screen titles.
    pub fn title(self) -> &'static str {
        match self {
            Role::HeartRate => "CARDIO",
            Role::SpeedCadence => "VITESSE / CADENCE",
            Role::Power => "PUISSANCE",
        }
    }
}

/// The sensors screens' content, filled by the firmware.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sensors {
    /// Each role's sensor, in [`Role::ALL`] order.
    pub roles: [SensorRow; 3],
    /// What the scan found for the role being paired, closest first.
    pub found: [Option<Found>; FOUND],
    pub calibration: Calibration,
}

/// The power meter's last calibration ([`Command::Calibrate`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Calibration {
    #[default]
    None,
    Running,
    /// Done: the offset found, in the meter's own units.
    Done(i16),
    Failed,
    /// The meter cannot do it.
    Unsupported,
    NotConnected,
    /// No answer in time.
    Timeout,
}

/// Sensors listed by a scan at most.
pub const FOUND: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Unpaired,
    /// Paired, not connected (asleep, away, out of battery).
    Searching,
    Connected,
}

/// A role's sensor: its link and what names it (its advertised name, else the end of its address).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorRow {
    pub link: Link,
    pub name: Option<Line>,
}

/// A sensor found by a scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: Line,
    pub rssi_dbm: i8,
    /// Already the role's sensor.
    pub paired: bool,
}

/// Colour of a key label: the main actions have their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    /// No action: "—" in the mute colour.
    Nothing,
    Go,
    Pause,
    Danger,
    Select,
}

/// What a key does now, written next to it; the icons are characters of the fonts ([`fonts::icon`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub text: &'static str,
    pub kind: Kind,
}

impl Hint {
    const fn new(text: &'static str) -> Self {
        Self {
            text,
            kind: Kind::Plain,
        }
    }

    const fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
}

const NOTHING: Hint = Hint::new("—").kind(Kind::Nothing);

/// The one banner shown, floating over the page (never over the key bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Banner {
    pub kind: BannerKind,
    pub title: &'static str,
    pub detail: Line,
    /// When it was shown, or a progress last moved (an information goes after [`INFO_MS`], a progress after
    /// [`PROGRESS_MS`] without news).
    pub since_ms: u64,
    /// For a progress, what is done, in thousandths: the bar under it. An information and an attention have no bar (a
    /// bar always means something moving on).
    pub bar_permille: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    /// Blue, for [`INFO_MS`], gone without a sign before.
    Info,
    /// Blue, with an arrow and a bar of what is done, while it moves (a file coming from the app).
    Progress,
    /// Amber, while it is true.
    Attention,
}

/// The project's page, on [`Screen::About`].
pub const ABOUT_URL: &str = "https://enavarro222.github.io/opencc600/";

/// The software version, the last `v*` git tag (`+N` commits after it), on the splash screen and [`Screen::About`]
/// (crates/ui/build.rs).
pub const VERSION: &str = env!("VERSION");

/// The commit the software is built from (crates/ui/build.rs), on [`Screen::About`].
pub const GIT_SHA: &str = env!("GIT_SHA");

/// The version and the commit, in the image's app descriptor (32 bytes at most): what `tools/flash.py --status`
/// shows of each slot.
pub const BUILD: &str = concat!(env!("VERSION"), " ", env!("GIT_SHA"));

/// A `Long` this soon after the last one of the same key is its repeat (the nRF sends one every ~200 ms).
pub const LONG_REPEAT_MS: u64 = 600;

/// The charging screen's animation: how long a cycle (the band climbing to the top, the dots rising a gap) lasts.
pub const CHARGE_CYCLE_MS: u64 = 1_200;

/// The automatic power off's banner title.
pub const AUTO_OFF: &str = "Extinction automatique";

/// The off-route banner's line alternates between the distance and how to set the route aside.
pub const OFF_ROUTE_HINT_MS: u64 = 4_000;
/// The off-route banner shows this long (the distance, then how to set the route aside), again when the distance
/// doubles; the top bar shows the distance in place of the clock meanwhile.
pub const OFF_ROUTE_ALERT_MS: u64 = 2 * OFF_ROUTE_HINT_MS;

/// Off the route: since when the banner shows, the distance then, the distance now (decimetres).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OffRoute {
    since_ms: u64,
    alert_dm: u32,
    off_dm: u32,
}

/// A distance off the route: in metres up to 2 km, then in km.
pub(crate) fn off_distance(dm: u32) -> Line {
    if dm < 20_000 {
        Line::new(format_args!("{} m", dm / 10))
    } else {
        Line::new(format_args!("{},{} km", dm / 10_000, dm / 1_000 % 10))
    }
}

/// Entries of a menu at most.
pub const MENU: usize = 6;

/// Entries of the menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    ResumeLast,
    /// The loaded route is followed: set it aside.
    FreeRide,
    /// The loaded route is set aside: follow it again.
    FollowRoute,
    Sensors,
    Rides,
    Status,
    About,
    PowerOff,
    /// Erase the paused ride (after confirmations).
    Discard,
}

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::ResumeLast => "\u{e00d} Reprendre la sortie",
            Item::Status => "État du compteur",
            Item::About => "À propos",
            Item::Sensors => "Capteurs",
            Item::Rides => "Sorties enregistrées",
            Item::FreeRide => "Rouler sans trace",
            Item::FollowRoute => "Suivre la trace",
            Item::PowerOff => "Éteindre (sortie gardée)",
            Item::Discard => "Abandonner la sortie",
        }
    }

    /// The entry opens a screen (a chevron says so); the others act at once.
    pub fn opens(self) -> bool {
        matches!(self, Item::Sensors | Item::Rides | Item::Status | Item::About)
    }

    /// The select key's label on this entry: a chevron when it opens a screen.
    pub fn action(self) -> &'static str {
        match self {
            Item::ResumeLast => "Reprendre",
            Item::FreeRide => "Sans trace",
            Item::FollowRoute => "Suivre",
            Item::Sensors | Item::Rides | Item::Status | Item::About => "Ouvrir \u{e00b}",
            Item::PowerOff => "Éteindre",
            Item::Discard => "Effacer…",
        }
    }
}

/// What the screens show, filled by the firmware before each [`draw`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    /// Local time, hours and minutes.
    pub clock: Option<(u8, u8)>,
    pub battery_pct: Option<u8>,
    /// On USB power: charging, or charged.
    pub plugged: bool,
    /// The phone's app (Musette) is connected over Bluetooth.
    pub phone: bool,
    pub gnss: Gnss,
    /// Current speed, 0 when stopped.
    pub speed_mm_s: Option<u32>,
    pub altitude_dm: Option<i32>,
    /// Gradient over the last 100 m, percent.
    pub grade_pct: Option<i8>,
    /// Climbing speed over the last minute of the timer, metres an hour ([`history::History::vam_m_h`]).
    pub vam_m_h: Option<i32>,
    pub heart_rate: Option<u8>,
    pub cadence_rpm: Option<u8>,
    pub power_w: Option<u16>,
    /// Average power of the last 3 s.
    pub power_3s_w: Option<u16>,
    /// The FTP and the max heart rate, the 100 % of their zones (0: not set, no zones).
    pub ftp_w: u16,
    pub hr_max: u16,
    pub power_zones: Zones,
    pub hr_zones: Zones,
    /// The speed from a wheel sensor.
    pub wheel_speed_mm_s: Option<u32>,
    pub sensors: Sensors,
    /// The ride in progress, or the last one finished (for [`Screen::Saved`]).
    pub ride: RideView,
    /// Altitude (dm) of the last 5 km, every 100 m, oldest first.
    pub recent: Profile,
    /// Altitude (dm) of the whole ride.
    pub whole: Profile,
    /// Finished rides on the device (all still to upload).
    pub rides_stored: Option<u16>,
    /// The ride is being saved ([`Screen::Saved`] says so until it is done).
    pub saving: bool,
    /// Hours of riding the free storage still holds (`recorder::hours_left`).
    pub record_hours: Option<u32>,
    /// The last finished ride can be resumed.
    pub resumable: bool,
    /// The route followed, and where the rider is on it.
    pub route: Option<RouteView>,
    /// A route is stored but set aside (a free ride).
    pub route_set_aside: bool,
    /// The nRF's sensor stream (barometer, clock) is missing: it only starts on the nRF's power-on gesture.
    pub stream_lost: bool,
    /// The ride in progress is no longer written to the storage.
    pub write_failure: Option<WriteFailure>,
    /// The ride in progress records a point every 5 s: the storage is nearly full.
    pub sparse: bool,
    /// The rides screen's content.
    pub rides: RidesView,
}

/// The rides screen's content, filled by the firmware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RidesView {
    /// Finished rides on the device.
    pub count: u16,
    /// The one selected ([`Ui::ride_item`]).
    pub card: Option<RideCard>,
    /// Room enough for a ride to start.
    pub can_start: bool,
}

/// A finished ride, as the rides screen tells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RideCard {
    /// Start, local time: day of the week (0 = Sunday), day, month (1 = January), hours, minutes.
    pub weekday: u8,
    pub day: u8,
    pub month: u8,
    pub hour: u8,
    pub minute: u8,
    pub timer_s: u32,
    pub distance_cm: u32,
    /// The companion app has it (acknowledged): deleting it loses nothing.
    pub sent: bool,
}

/// Why the ride in progress is no longer written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteFailure {
    /// No room left, even without the logs.
    Full,
    /// Another storage error.
    Error,
}

/// The route loaded: what the home screen tells, where the map is drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteView {
    pub name: Line,
    pub length_dm: u32,
    pub ascent_m: u16,
    /// The rider on the route's plane (decimetres) with a fix.
    pub me: Option<(i32, i32)>,
    /// Course over ground, hundredths of a degree (the last one while stopped).
    pub heading_cdeg: Option<u16>,
    pub progress: Option<route::Progress>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gnss {
    #[default]
    Off,
    /// `progress`: how far the acquisition probably is, percent.
    Searching {
        in_view: u8,
        progress: u8,
    },
    Fix {
        used: u8,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RideView {
    /// Time with the timer running, excluding manual and automatic pauses.
    pub timer_s: u32,
    /// The ride is active but its timer is automatically paused.
    pub auto_paused: bool,
    pub elapsed_s: u32,
    pub distance_cm: u32,
    pub ascent_m: u16,
    pub descent_m: u16,
    pub max_speed_mm_s: u32,
    pub avg_power_w: Option<u16>,
    pub avg_heart_rate: Option<u8>,
    pub max_heart_rate: Option<u8>,
}

/// A ride shorter than both is empty: started by mistake, it is erased without more confirmation.
pub const EMPTY_RIDE_CM: u32 = 10_000;
pub const EMPTY_RIDE_S: u32 = 180;

impl RideView {
    pub fn empty(&self) -> bool {
        self.distance_cm < EMPTY_RIDE_CM && self.timer_s < EMPTY_RIDE_S
    }

    pub fn avg_speed_mm_s(&self) -> Option<u32> {
        (self.timer_s > 0).then(|| (self.distance_cm as u64 * 10 / self.timer_s as u64) as u32)
    }
}

/// Altitude points, oldest first, `len` of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub points: [i16; PROFILE],
    pub len: u8,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            points: [0; PROFILE],
            len: 0,
        }
    }
}

impl Profile {
    pub fn points(&self) -> &[i16] {
        &self.points[..self.len as usize]
    }
}

/// A zone shown, which follows the value's zone once it stayed there [`ZONE_HOLD_MS`]; at once from no zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    shown: Option<u8>,
    /// Another zone the value is in, since when.
    next: Option<(u8, u64)>,
}

impl Held {
    const fn new() -> Self {
        Self { shown: None, next: None }
    }

    fn update(&mut self, zone: Option<u8>, now_ms: u64) {
        match (zone, self.shown) {
            (None, _) | (Some(_), None) => *self = Self { shown: zone, next: None },
            (Some(z), Some(shown)) if z == shown => self.next = None,
            (Some(z), Some(_)) => match self.next {
                Some((n, since)) if n == z => {
                    if now_ms.saturating_sub(since) >= ZONE_HOLD_MS {
                        *self = Self { shown: zone, next: None };
                    }
                }
                _ => self.next = Some((z, now_ms)),
            },
        }
    }
}

/// Which screen is shown and what the keys do on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    screen: Screen,
    /// Where the menus, the status and the confirmation go back to.
    back: Screen,
    pages: Pages,
    page: u8,
    item: u8,
    /// The role selected on the sensors screen, and paired on the scan screen.
    role: u8,
    /// On the sensors screen, the power meter's calibration entry (after the roles) is selected.
    calibrate_row: bool,
    /// Where the charging screen's animation is in its cycle, in ms.
    charge_ms: u16,
    /// Automatic power off after this long without moving during a ride (0: never), and since when it is still.
    auto_off_ms: u64,
    still_since: Option<u64>,
    /// The key held long, and when its last `Long` came: the repeats are dropped.
    held: Option<(Key, u64)>,
    scroll: bool,
    scroll_ms: u64,
    scrolled_at: u64,
    banner: Option<Banner>,
    /// Map zoom, index in [`ZOOMS_M`], or the whole route after them.
    zoom: u8,
    /// What picks the pages: a route is loaded, it has climbs, the usage is training.
    route: bool,
    climbs: bool,
    train: bool,
    effort: Effort,
    /// The power and heart rate zones shown.
    power_zone: Held,
    hr_zone: Held,
    /// The GNSS had a fix during this ride ("lost" rather than "searching").
    had_fix: bool,
    fix: bool,
    off_route: Option<OffRoute>,
    /// The entry of the rides screen, and whether it was opened because a ride could not start.
    ride_item: u16,
    making_room: bool,
    model: &'static str,
    ble_name: &'static str,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub const fn new() -> Self {
        Self {
            screen: Screen::Home,
            back: Screen::Home,
            pages: Pages {
                list: [None; MAX_PAGES],
            },
            page: 0,
            item: 0,
            role: 0,
            calibrate_row: false,
            charge_ms: 0,
            auto_off_ms: 0,
            still_since: None,
            held: None,
            scroll: false,
            scroll_ms: SCROLL_MS,
            scrolled_at: 0,
            banner: None,
            zoom: 2,
            route: false,
            climbs: false,
            train: false,
            effort: Effort::None,
            power_zone: Held::new(),
            hr_zone: Held::new(),
            had_fix: false,
            fix: false,
            off_route: None,
            ride_item: 0,
            making_room: false,
            model: "CC600",
            ble_name: "",
        }
    }

    /// The model (`CC600`, `CC700`) and the Bluetooth name, on the splash screen.
    pub fn set_identity(&mut self, model: &'static str, ble_name: &'static str) {
        self.model = model;
        self.ble_name = ble_name;
    }

    pub fn model(&self) -> &'static str {
        self.model
    }

    pub fn ble_name(&self) -> &'static str {
        self.ble_name
    }

    /// Automatic power off during a ride after `min` minutes without moving (the `auto_off_min` setting; 0: never).
    pub fn set_auto_off_min(&mut self, min: u8) {
        self.auto_off_ms = min as u64 * 60_000;
    }

    /// How long before the automatic power off its banner shows: a minute, half the delay when it is short.
    fn auto_off_warning_ms(&self) -> u64 {
        (self.auto_off_ms / 2).min(60_000)
    }

    /// The automatic power off is announced: what is left of its delay, ms.
    fn auto_off_left(&self, now_ms: u64) -> Option<u64> {
        let still = now_ms.saturating_sub(self.still_since?);
        let left = self.auto_off_ms.checked_sub(still)?;
        (self.auto_off_ms > 0 && left <= self.auto_off_warning_ms()).then_some(left)
    }

    /// Auto-scroll interval (the `scroll_s` setting).
    pub fn set_scroll_s(&mut self, s: u8) {
        self.scroll_ms = s as u64 * 1000;
    }

    /// What picks the ride pages (at the next start): a route is loaded, it has climbs, the usage setting is training,
    /// a power meter or a heart rate sensor is paired. The pages of a ride follow a route sent or removed and a sensor paired or
    /// forgotten during it (and a ride resumed at power-on, before the route was read), the page shown kept.
    pub fn set_context(&mut self, route: bool, climbs: bool, train: bool, effort: Effort) {
        let changed = route != self.route || climbs != self.climbs || effort != self.effort;
        self.route = route;
        self.climbs = climbs;
        self.train = train;
        self.effort = effort;
        if changed && !self.pages.is_empty() {
            let shown = self.page();
            self.pages = self.next_pages();
            self.page = (0..self.pages.len())
                .find(|&i| self.pages.get(i) == shown)
                .unwrap_or(0) as u8;
        }
    }

    /// The pages a ride would have now.
    pub fn next_pages(&self) -> Pages {
        pages(self.route, self.climbs, self.train, self.effort)
    }

    /// The map's scale bar, metres.
    /// `None`: the whole route.
    pub fn zoom_m(&self) -> Option<u32> {
        ZOOMS_M.get(self.zoom as usize).copied()
    }

    /// The logo at power-on, until [`Ui::end_splash`]; then the screen shown now (home, or the paused ride found).
    pub fn show_splash(&mut self) {
        if self.screen != Screen::Splash {
            self.back = self.screen;
            self.screen = Screen::Splash;
        }
    }

    pub fn end_splash(&mut self) {
        if self.screen == Screen::Splash {
            self.screen = self.back;
        }
    }

    /// The charging screen, until a long press of the power key; then back to the screen shown now (home, or the
    /// paused ride found at power-on).
    pub fn show_charging(&mut self) {
        if self.screen != Screen::Charging {
            self.back = self.screen;
            self.screen = Screen::Charging;
        }
    }

    /// Where the charging screen's animation is in its cycle, 0..[`CHARGE_CYCLE_MS`] ms.
    pub fn charge_ms(&self) -> u16 {
        self.charge_ms
    }

    /// At power-on with a paused ride: the screen that offers to resume or finish it.
    pub fn found_paused_ride(&mut self) {
        self.screen = Screen::Resume;
        self.pages = self.next_pages();
    }

    /// No room for the ride asked for: the rides screen, the first entry selected (the firmware lists the ones the
    /// app has first), to delete some; it offers to start once there is room.
    pub fn make_room(&mut self) {
        self.screen = Screen::Rides;
        self.ride_item = 0;
        self.making_room = true;
    }

    /// The entry selected on the rides screen.
    pub fn ride_item(&self) -> u16 {
        self.ride_item
    }

    /// The rides screen was opened because a ride could not start.
    pub fn making_room(&self) -> bool {
        self.making_room
    }

    /// The role selected on [`Screen::Sensors`] or paired on [`Screen::SensorScan`].
    pub fn role(&self) -> Role {
        Role::ALL[self.role as usize]
    }

    /// On [`Screen::Sensors`]: the power meter's calibration entry is selected (shown once it is paired).
    pub fn calibrate_row(&self) -> bool {
        self.calibrate_row
    }

    /// On [`Screen::SensorScan`]: the entry selected, in the found sensors then "none" when the role is paired.
    pub fn scan_item(&self) -> u8 {
        self.item
    }

    /// The entries of the scan screen: the sensors found, then "none" (forget) if the role has a sensor.
    fn scan_entries(&self, view: &View) -> (u8, bool) {
        let found = view.sensors.found.iter().flatten().count() as u8;
        let forget = view.sensors.roles[self.role as usize].link != Link::Unpaired;
        (found, forget)
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn page(&self) -> Option<Page> {
        self.pages.get(self.page as usize)
    }

    /// The page shown and how many there are.
    /// The power zone shown (1 the first), [`ZONE_HOLD_MS`] after the 3 s power got into it; `None` without an FTP
    /// or a power.
    pub fn power_zone(&self) -> Option<u8> {
        self.power_zone.shown
    }

    /// The heart rate zone shown, as [`Ui::power_zone`].
    pub fn hr_zone(&self) -> Option<u8> {
        self.hr_zone.shown
    }

    pub fn page_index(&self) -> (u8, u8) {
        (self.page, self.pages.len() as u8)
    }

    pub fn scrolling(&self) -> bool {
        self.scroll
    }

    pub fn banner(&self) -> Option<&Banner> {
        self.banner.as_ref()
    }

    /// Shows an information for [`INFO_MS`] (an attention stays first).
    pub fn inform(&mut self, title: &'static str, detail: Line, now_ms: u64) {
        self.show(BannerKind::Info, title, detail, 0, now_ms);
    }

    /// Shows how far something has come, `done` of `total`, while it moves (an attention stays first). The banner
    /// goes [`PROGRESS_MS`] after the last call; an information (the end) replaces it.
    pub fn progress(&mut self, title: &'static str, detail: Line, done: u32, total: u32, now_ms: u64) {
        let permille = (u64::from(done.min(total)) * 1000 / u64::from(total.max(1))) as u16;
        self.show(BannerKind::Progress, title, detail, permille, now_ms);
    }

    fn show(&mut self, kind: BannerKind, title: &'static str, detail: Line, bar_permille: u16, now_ms: u64) {
        if !self.banner.is_some_and(|b| b.kind == BannerKind::Attention) {
            self.banner = Some(Banner { kind, title, detail, since_ms: now_ms, bar_permille });
        }
    }

    /// The entries of the menu shown, and the selected one.
    pub fn menu(&self, view: &View) -> ([Option<Item>; MENU], u8) {
        // follow the route or set it aside, when one is stored
        let route = match (view.route.is_some(), view.route_set_aside) {
            (true, _) => Some(Item::FreeRide),
            (false, true) => Some(Item::FollowRoute),
            _ => None,
        };
        let items: &[Option<Item>] = match self.screen {
            Screen::Menu => &[
                view.resumable.then_some(Item::ResumeLast),
                route,
                Some(Item::Sensors),
                Some(Item::Rides),
                Some(Item::Status),
                Some(Item::About),
            ],
            // a strap that drops during a ride is paired again from the pause, the route set aside there too
            // abandoning last, the furthest from the first entry
            Screen::PauseMenu => &[
                Some(Item::Sensors),
                route,
                Some(Item::PowerOff),
                Some(Item::Status),
                Some(Item::Discard),
            ],
            _ => &[],
        };
        let mut out = [None; MENU];
        for (o, item) in out.iter_mut().zip(items.iter().flatten()) {
            *o = Some(*item);
        }
        (out, self.item)
    }

    pub fn key(&mut self, key: Key, press: Press, now_ms: u64, view: &View) -> Option<Command> {
        use Key::*;
        use Press::*;
        use Screen::*;
        // a key while the automatic power off is announced only cancels it; any key restarts its delay
        let announced = self.auto_off_left(now_ms).is_some();
        if self.still_since.is_some() {
            self.still_since = Some(now_ms);
        }
        // still held: the charging screen's wake up must not become the home screen's power off
        if press == Long {
            let repeat = self.held.is_some_and(|(k, at)| k == key && now_ms.saturating_sub(at) < LONG_REPEAT_MS);
            self.held = Some((key, now_ms));
            if repeat {
                return None;
            }
        } else {
            self.held = None;
        }
        if announced {
            if self.banner.is_some_and(|b| b.title == AUTO_OFF) {
                self.banner = None;
            }
            return None;
        }
        let (screen, command) = match (self.screen, key, press) {
            // without the nRF's sensor stream, the long press starts it (the nRF does on this gesture): not a power off
            (Home, TopLeft, Long) if view.stream_lost => {
                self.inform("Capteurs relancés", Line::new(format_args!("Baromètre et horloge")), now_ms);
                (Home, None)
            }
            (Home, TopLeft, Long) => (Home, Some(Command::PowerOff)),
            (Charging, TopLeft, Long) => (Splash, Some(Command::WakeUp)),
            (Home, BottomLeft, Click) => {
                self.item = 0;
                self.back = Home;
                (Menu, None)
            }
            (Home, BottomRight, Click) => {
                self.start_ride();
                (Ride, Some(Command::Start))
            }
            (Menu | PauseMenu, TopLeft, Click) => (self.back, None),
            (Menu | PauseMenu, BottomLeft, Click) => {
                let n = self.menu(view).0.iter().flatten().count() as u8;
                self.item = (self.item + 1) % n.max(1);
                (self.screen, None)
            }
            (Menu | PauseMenu, BottomRight, Click) => {
                match self.menu(view).0[self.item as usize] {
                    Some(Item::ResumeLast) => {
                        self.start_ride();
                        (Ride, Some(Command::ResumeLast))
                    }
                    // the status goes back to its menu, the menu to where it was opened from
                    Some(Item::Status) => (Status, None),
                    Some(Item::About) => (About, None),
                    Some(Item::Sensors) => {
                        self.calibrate_row = false;
                        (Sensors, None)
                    }
                    Some(Item::Rides) => {
                        self.ride_item = 0;
                        self.making_room = false;
                        (Rides, None)
                    }
                    // back where the menu was opened from: the change shows there
                    Some(Item::FreeRide) => (self.back, Some(Command::FollowRoute(false))),
                    Some(Item::FollowRoute) => (self.back, Some(Command::FollowRoute(true))),
                    Some(Item::PowerOff) => (self.screen, Some(Command::PowerOff)),
                    Some(Item::Discard) if view.ride.empty() => (ConfirmEmpty, None),
                    Some(Item::Discard) => (ConfirmDiscard, None),
                    None => (self.screen, None),
                }
            }
            (Status | Sensors | About, TopLeft, Click) => (if self.back == Home { Menu } else { PauseMenu }, None),
            // after the power meter, its calibration once it is paired
            (Sensors, BottomLeft, Click) => {
                if self.calibrate_row {
                    self.calibrate_row = false;
                    self.role = 0;
                } else if self.role() == Role::Power && view.sensors.roles[2].link != Link::Unpaired {
                    self.calibrate_row = true;
                } else {
                    self.role = (self.role + 1) % Role::ALL.len() as u8;
                }
                (Sensors, None)
            }
            (Sensors, BottomRight, Click) if self.calibrate_row => (Calibrate, None),
            (Sensors, BottomRight, Click) => {
                self.item = 0;
                (SensorScan, Some(Command::Scan(self.role())))
            }
            (SensorScan, TopLeft, Click) => (Sensors, Some(Command::StopScan)),
            (Calibrate, TopLeft, Click) => (Sensors, None),
            (Calibrate, BottomRight, Click) if view.sensors.calibration != Calibration::Running => {
                (Calibrate, Some(Command::Calibrate))
            }
            (SensorScan, BottomLeft, Click) => {
                let (found, forget) = self.scan_entries(view);
                self.item = (self.item + 1) % (found + forget as u8).max(1);
                (SensorScan, None)
            }
            (SensorScan, BottomRight, Click) => match self.scan_entries(view) {
                (found, _) if self.item < found => (Sensors, Some(Command::Pair(self.role(), self.item))),
                (found, true) if self.item == found => (Sensors, Some(Command::Forget(self.role()))),
                _ => (SensorScan, None),
            },
            // a page changed or zoomed by hand stays a full interval before the next scroll
            (Ride, TopLeft, Click) if self.page() == Some(Page::Map) => {
                self.zoom = (self.zoom + 1) % ZOOM_STEPS;
                self.scrolled_at = now_ms;
                (Ride, None)
            }
            (Ride, TopLeft, Click) => {
                self.scroll = !self.scroll;
                self.scrolled_at = now_ms;
                (Ride, None)
            }
            (Ride, BottomLeft, Click) => {
                self.next_page();
                self.scrolled_at = now_ms;
                (Ride, None)
            }
            (Ride, BottomRight, Click) => {
                self.scroll = false;
                (Paused, Some(Command::Pause))
            }
            (Paused | Resume, TopLeft, Click) => {
                self.back = self.screen;
                self.item = 0;
                (PauseMenu, None)
            }
            (Paused | Resume, BottomLeft, Click) => {
                self.back = self.screen;
                (if view.ride.empty() { ConfirmEmpty } else { ConfirmStop }, None)
            }
            (Paused | Resume, BottomRight, Click) => (Ride, Some(Command::Resume)),
            (ConfirmStop | ConfirmEmpty | ConfirmDiscard | ConfirmDiscardAgain, TopLeft, Click) => (self.back, None),
            (ConfirmStop | ConfirmDiscard, BottomLeft, Click) => (self.back, None),
            (ConfirmStop, BottomRight, Click) | (ConfirmEmpty, BottomLeft, Click) => (Saved, Some(Command::Stop)),
            (ConfirmEmpty, BottomRight, Click) | (ConfirmDiscardAgain, BottomLeft, Click) => {
                (Home, Some(Command::Discard))
            }
            (ConfirmDiscard, BottomRight, Click) => (ConfirmDiscardAgain, None),
            (ConfirmDiscardAgain, BottomRight, Click) => (self.back, None),
            (Saved, BottomLeft, Click) if view.resumable => {
                self.start_ride();
                (Ride, Some(Command::ResumeLast))
            }
            (Saved, BottomRight, Click) => (Home, None),
            (Rides, TopLeft, Click) => (if self.making_room { Home } else { Menu }, None),
            (Rides, BottomLeft, Click) if view.rides.count > 1 => {
                self.ride_item = (self.ride_item + 1) % view.rides.count;
                (Rides, None)
            }
            (Rides, BottomRight, Click) if self.making_room && view.rides.can_start => {
                self.making_room = false;
                self.start_ride();
                (Ride, Some(Command::Start))
            }
            (Rides, BottomRight, Click) if view.rides.card.is_some() => (ConfirmDelete, None),
            (ConfirmDelete, TopLeft | BottomLeft, Click) => (Rides, None),
            (ConfirmDelete, BottomRight, Click) => (Rides, Some(Command::DeleteRide(self.ride_item))),
            (s, _, _) => (s, None),
        };
        self.screen = screen;
        command
    }

    fn start_ride(&mut self) {
        self.pages = self.next_pages();
        self.page = 0;
        self.had_fix = false;
    }

    fn next_page(&mut self) {
        self.page = (self.page + 1) % (self.pages.len() as u8).max(1);
    }

    /// Follows the ride (`None` without one, else whether it is active, including auto-pause) and the GNSS fix;
    /// scrolls the pages, sets and clears the banners. [`Command::PowerOff`] when the device stayed still during a
    /// ride for the automatic power off's delay (the firmware pauses and keeps the ride).
    pub fn tick(&mut self, now_ms: u64, ride: Option<bool>, view: &View) -> Option<Command> {
        use Screen::*;
        if self.screen == Charging {
            self.charge_ms = (now_ms % CHARGE_CYCLE_MS) as u16;
        }
        self.power_zone.update(
            view.power_3s_w.and_then(|w| view.power_zones.zone(w, view.ftp_w)),
            now_ms,
        );
        self.hr_zone.update(
            view.heart_rate.and_then(|bpm| view.hr_zones.zone(bpm as u16, view.hr_max)),
            now_ms,
        );
        // a ride deleted: the next one takes its place, the last one when it was the last
        self.ride_item = self.ride_item.min(view.rides.count.saturating_sub(1));
        // still: no speed (GNSS or wheel), no pedalling; without a fix (indoors, a café) it is still too
        let moving = view.speed_mm_s.is_some_and(|v| v > 0)
            || view.wheel_speed_mm_s.is_some_and(|v| v > 0)
            || view.cadence_rpm.is_some_and(|c| c > 0)
            || view.power_w.is_some_and(|w| w > 0);
        self.still_since = match (ride.is_some() && self.auto_off_ms > 0, moving, self.still_since) {
            (false, _, _) | (true, true, _) => None,
            (true, false, since) => Some(since.unwrap_or(now_ms)),
        };
        let power_off = self
            .still_since
            .is_some_and(|since| now_ms.saturating_sub(since) >= self.auto_off_ms);
        if power_off {
            self.still_since = None;
        }
        let confirming = matches!(self.screen, ConfirmStop | ConfirmEmpty | ConfirmDiscard | ConfirmDiscardAgain);
        self.screen = match (self.screen, ride) {
            (Ride | Paused | PauseMenu | Resume, None) => Home,
            (_, None) if confirming => Home,
            (Home | Saved | Paused | Resume, Some(true)) => Ride,
            (_, Some(true)) if confirming => Ride,
            (Ride, Some(false)) => Paused,
            (s, _) => s,
        };
        if self.scroll
            && self.screen == Ride
            && now_ms.saturating_sub(self.scrolled_at) >= self.scroll_ms
        {
            self.next_page();
            self.scrolled_at = now_ms;
        }
        // Attention banners while riding: the GNSS searching or lost while there is no fix, then off the route;
        // when the GNSS comes back, an information for a while.
        let fix = matches!(view.gnss, Gnss::Fix { .. });
        let riding = ride.is_some();
        let off_dm = view.route.and_then(|r| r.progress).map(|p| p.off_dm);
        // kept while the fix is lost; the banner again when the distance doubles
        self.off_route = match (off_dm, self.off_route) {
            _ if !riding => None,
            (_, o) if !fix => o,
            (Some(d), Some(o)) if d >= OFF_ROUTE_DM / 2 && d < 2 * o.alert_dm => Some(OffRoute { off_dm: d, ..o }),
            (Some(d), _) if d > OFF_ROUTE_DM => Some(OffRoute { since_ms: now_ms, alert_dm: d, off_dm: d }),
            _ => None,
        };
        let off_route = self.off_route.filter(|o| now_ms.saturating_sub(o.since_ms) < OFF_ROUTE_ALERT_MS);
        let attention = match (riding, fix, off_route) {
            _ if self.auto_off_left(now_ms).is_some() => {
                let left_s = self.auto_off_left(now_ms).unwrap_or(0).div_ceil(1000);
                Some((AUTO_OFF, Line::new(format_args!("Dans {left_s} s, une touche annule"))))
            }
            // first: the ride goes on but is no longer recorded
            (true, ..) if view.write_failure.is_some() => {
                let title = match view.write_failure {
                    Some(WriteFailure::Full) => "Mémoire pleine",
                    _ => "Erreur d'écriture",
                };
                Some((title, Line::new(format_args!("La sortie n'est plus enregistrée"))))
            }
            (true, false, _) if self.had_fix => Some(("GPS perdu", Line::new(format_args!("Distance en attente du GPS")))),
            (true, false, _) => Some(("Recherche du GPS", Line::new(format_args!("La distance part au fix")))),
            // the distance, then: the wrong route followed by mistake is set aside from the pause menu (the device
            // never drops it)
            (true, true, Some(o)) if now_ms - o.since_ms >= OFF_ROUTE_HINT_MS => {
                Some(("Hors de la trace", Line::new(format_args!("Pas cette trace ? Pause › Menu"))))
            }
            (true, true, Some(o)) => {
                Some(("Hors de la trace", Line::new(format_args!("à {}", off_distance(o.off_dm).as_str()))))
            }
            (true, ..) if view.ride.auto_paused => {
                Some(("Pause automatique", Line::new(format_args!("Reprise dès que tu roules"))))
            }
            _ => None,
        };
        match (attention, self.banner) {
            (Some((title, detail)), Some(b)) if b.title == title => {
                self.banner = Some(Banner { detail, ..b });
            }
            (Some((title, detail)), _) => {
                self.banner = Some(Banner {
                    kind: BannerKind::Attention,
                    title,
                    detail,
                    since_ms: now_ms,
                    bar_permille: 0,
                });
            }
            (None, Some(b)) if b.kind == BannerKind::Attention => {
                self.banner = None;
                match (riding, b.title, view.gnss) {
                    (true, "GPS perdu", Gnss::Fix { used }) => {
                        self.inform("GPS retrouvé", Line::new(format_args!("{used} satellites")), now_ms)
                    }
                    (true, "Mémoire pleine" | "Erreur d'écriture", _) => {
                        self.inform("Enregistrement repris", Line::new(format_args!("")), now_ms)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if riding && fix {
            self.had_fix = true;
        }
        self.fix = fix;
        if let Some(b) = self.banner {
            let age = now_ms.saturating_sub(b.since_ms);
            match b.kind {
                BannerKind::Info if age >= INFO_MS => self.banner = None,
                BannerKind::Progress if age >= PROGRESS_MS => self.banner = None,
                _ => {}
            }
        }
        power_off.then_some(Command::PowerOff)
    }

    /// Labels of the top left, bottom left and bottom right keys.
    pub fn hints(&self, view: &View) -> [Hint; 3] {
        use Screen::*;
        const BACK: Hint = Hint::new("\u{e003} Retour");
        const NEXT: Hint = Hint::new("Suivant \u{e00a}");
        const MENU: Hint = Hint::new("\u{e009} Menu");
        match self.screen {
            Home => [
                Hint::new("\u{e005} Maintenir"),
                MENU,
                Hint::new("Démarrer").kind(Kind::Go),
            ],
            Menu | PauseMenu => {
                let (items, at) = self.menu(view);
                match items[at as usize] {
                    Some(Item::Discard) => [BACK, NEXT, Hint::new(Item::Discard.action()).kind(Kind::Danger)],
                    Some(item) => [BACK, NEXT, Hint::new(item.action()).kind(Kind::Select)],
                    None => [BACK, NEXT, NOTHING],
                }
            }
            Status | About => [BACK, NOTHING, NOTHING],
            Charging | Splash => [NOTHING, NOTHING, NOTHING],
            Sensors if self.calibrate_row => [BACK, NEXT, Hint::new("Ouvrir \u{e00b}").kind(Kind::Select)],
            Sensors => [BACK, NEXT, Hint::new("Chercher \u{e00b}").kind(Kind::Select)],
            Calibrate if view.sensors.calibration == Calibration::Running => [BACK, NOTHING, NOTHING],
            Calibrate => [BACK, NOTHING, Hint::new("Calibrer").kind(Kind::Go)],
            SensorScan => match self.scan_entries(view) {
                (0, false) => [BACK, NOTHING, NOTHING],
                (found, forget) => [
                    BACK,
                    if found + forget as u8 > 1 { NEXT } else { NOTHING },
                    Hint::new("Choisir \u{e00b}").kind(Kind::Select),
                ],
            },
            // the map's top left key is its scale: the length of the bar drawn next to it ([`Ui::map_scale`]);
            // without a position the whole route is shown, at no fixed scale
            Ride if self.page() == Some(Page::Map) => [
                match (self.map_scale(view), self.zoom_m()) {
                    (Some(_), Some(_)) => Hint::new(["125 m", "250 m", "500 m", "1 km"][self.zoom as usize]),
                    (_, None) => Hint::new("\u{e00f} Trace"),
                    (None, Some(_)) => Hint::new("\u{e00f} Zoom"),
                },
                Hint::new("Page \u{e004}"),
                Hint::new("Pause \u{e001}").kind(Kind::Pause),
            ],
            Ride => [
                Hint::new(if self.scroll {
                    "\u{e00c} Défil. oui"
                } else {
                    "\u{e00c} Défil. non"
                }),
                Hint::new("Page \u{e004}"),
                Hint::new("Pause \u{e001}").kind(Kind::Pause),
            ],
            Paused | Resume => [
                MENU,
                Hint::new("Terminer…"),
                Hint::new("Reprendre").kind(Kind::Go),
            ],
            ConfirmStop => [
                Hint::new("\u{e003} Annuler"),
                Hint::new("Annuler"),
                Hint::new("Terminer \u{e002}").kind(Kind::Select),
            ],
            ConfirmEmpty => [
                Hint::new("\u{e003} Annuler"),
                Hint::new("Enregistrer"),
                Hint::new("Effacer \u{e007}").kind(Kind::Danger),
            ],
            ConfirmDiscard => [
                Hint::new("\u{e003} Annuler"),
                Hint::new("Annuler"),
                Hint::new("Effacer…").kind(Kind::Danger),
            ],
            ConfirmDiscardAgain => [
                Hint::new("\u{e003} Annuler"),
                Hint::new("Effacer \u{e007}").kind(Kind::Danger),
                Hint::new("Garder").kind(Kind::Go),
            ],
            Rides => {
                // the list from the menu is the latest first, the one to make room the ones to delete first
                let next = match view.rides.count {
                    0 | 1 => NOTHING,
                    _ if self.making_room => Hint::new("Suivante \u{e00a}"),
                    _ => Hint::new("Précédente"),
                };
                match view.rides.card {
                    _ if self.making_room && view.rides.can_start => [BACK, next, Hint::new("Démarrer").kind(Kind::Go)],
                    Some(_) => [BACK, next, Hint::new("Supprimer…").kind(Kind::Danger)],
                    None => [BACK, NOTHING, NOTHING],
                }
            }
            ConfirmDelete => [
                Hint::new("\u{e003} Annuler"),
                Hint::new("Annuler"),
                Hint::new("Supprimer").kind(Kind::Danger),
            ],
            // no key acts while the ride is being saved
            Saved if view.saving => [NOTHING, NOTHING, NOTHING],
            Saved => [
                NOTHING,
                if view.resumable {
                    Hint::new("Reprendre")
                } else {
                    NOTHING
                },
                Hint::new("OK").kind(Kind::Select),
            ],
        }
    }

    /// The map's scale bar in the top left key, pixels (for the length in its label), when the map is shown around
    /// the rider.
    pub fn map_scale(&self, view: &View) -> Option<i32> {
        (self.screen == Screen::Ride
            && self.page() == Some(Page::Map)
            && self.zoom_m().is_some()
            && view.route.is_some_and(|r| r.me.is_some()))
        .then_some(map::SCALE_PX as i32 / 2)
    }

    /// The top bar shows "GPS ✕" in place of the clock (riding without a fix).
    pub fn gps_warning(&self) -> bool {
        matches!(self.screen, Screen::Ride | Screen::Paused) && !self.fix
    }

    /// The top bar shows the distance off the route in place of the clock, decimetres (riding off the route).
    pub fn off_route_dm(&self) -> Option<u32> {
        self.off_route.filter(|_| matches!(self.screen, Screen::Ride | Screen::Paused) && self.fix).map(|o| o.off_dm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::*;
    use Press::*;

    fn view() -> View {
        View {
            gnss: Gnss::Fix { used: 9 },
            ride: RideView { timer_s: 3600, distance_cm: 2_500_000, ..Default::default() },
            ..Default::default()
        }
    }

    fn press(ui: &mut Ui, key: Key) -> Option<Command> {
        ui.key(key, Click, 0, &view())
    }

    #[test]
    fn an_empty_ride_is_erased_from_the_finish_key() {
        let empty = View { ride: RideView { timer_s: 150, distance_cm: 4_000, ..Default::default() }, ..view() };
        let mut ui = Ui::new();
        ui.key(BottomRight, Click, 0, &empty);
        ui.key(BottomRight, Click, 0, &empty);
        ui.key(BottomLeft, Click, 0, &empty);
        assert_eq!(ui.screen(), Screen::ConfirmEmpty);
        // saved all the same from the bottom left
        let mut save = ui;
        assert_eq!(save.key(BottomLeft, Click, 0, &empty), Some(Command::Stop));
        assert_eq!(save.screen(), Screen::Saved);
        assert_eq!(ui.key(BottomRight, Click, 0, &empty), Some(Command::Discard));
        assert_eq!(ui.screen(), Screen::Home);
        // 3 min or 100 m make it a ride
        for ride in [RideView { timer_s: 180, ..empty.ride }, RideView { distance_cm: 10_000, ..empty.ride }] {
            let mut ui = Ui::new();
            ui.found_paused_ride();
            ui.key(BottomLeft, Click, 0, &View { ride, ..empty });
            assert_eq!(ui.screen(), Screen::ConfirmStop);
        }
    }

    #[test]
    fn a_ride_is_abandoned_from_the_pause_menu_after_two_confirmations() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, TopLeft);
        // the last entry
        for _ in 0..3 {
            press(&mut ui, BottomLeft);
        }
        assert_eq!(ui.menu(&view()).0[ui.item as usize], Some(Item::Discard));
        assert_eq!(press(&mut ui, BottomRight), None);
        assert_eq!(ui.screen(), Screen::ConfirmDiscard);
        assert_eq!(press(&mut ui, BottomRight), None);
        assert_eq!(ui.screen(), Screen::ConfirmDiscardAgain);
        // pressing on out of habit keeps the ride
        let mut habit = ui;
        assert_eq!(press(&mut habit, BottomRight), None);
        assert_eq!(habit.screen(), Screen::Paused);
        let mut cancel = ui;
        press(&mut cancel, TopLeft);
        assert_eq!(cancel.screen(), Screen::Paused);
        assert_eq!(press(&mut ui, BottomLeft), Some(Command::Discard));
        assert_eq!(ui.screen(), Screen::Home);
    }

    #[test]
    fn erasing_is_cancelled_back_to_the_pause_it_came_from() {
        let empty = View { ride: RideView::default(), ..view() };
        // the pause menu's last entry, abandoning, then each confirmation
        const MENU: &[Key] = &[TopLeft, BottomLeft, BottomLeft, BottomLeft, BottomRight];
        const AGAIN: &[Key] = &[TopLeft, BottomLeft, BottomLeft, BottomLeft, BottomRight, BottomRight];
        for (from, v, keys, confirm) in [
            (Screen::Paused, empty, &[BottomLeft][..], Screen::ConfirmEmpty),
            (Screen::Resume, empty, &[BottomLeft], Screen::ConfirmEmpty),
            (Screen::Paused, view(), MENU, Screen::ConfirmDiscard),
            (Screen::Resume, view(), MENU, Screen::ConfirmDiscard),
            (Screen::Paused, view(), AGAIN, Screen::ConfirmDiscardAgain),
            // the menu entry on an empty ride: its one confirmation
            (Screen::Paused, empty, MENU, Screen::ConfirmEmpty),
        ] {
            let mut ui = Ui::new();
            if from == Screen::Resume {
                ui.found_paused_ride();
            } else {
                ui.key(BottomRight, Click, 0, &v);
                ui.key(BottomRight, Click, 0, &v);
            }
            for &k in keys {
                assert_eq!(ui.key(k, Click, 0, &v), None);
            }
            assert_eq!(ui.screen(), confirm, "{from:?} {keys:?}");
            // every confirmation cancels from the top left
            let cancels: &[Key] = match confirm {
                // the bottom left saves, or erases
                Screen::ConfirmEmpty | Screen::ConfirmDiscardAgain => &[TopLeft],
                _ => &[TopLeft, BottomLeft],
            };
            for &k in cancels {
                let mut cancel = ui;
                assert_eq!(cancel.key(k, Click, 0, &v), None);
                assert_eq!(cancel.screen(), from, "{confirm:?} {k:?}");
            }
            // the ride gone meanwhile (finished from the app), or running again: the screen follows
            let mut gone = ui;
            gone.tick(0, None, &v);
            assert_eq!(gone.screen(), Screen::Home);
            let mut running = ui;
            running.tick(0, Some(true), &v);
            assert_eq!(running.screen(), Screen::Ride);
        }
    }

    #[test]
    fn the_pause_menu_names_abandoning_in_the_danger_colour() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, TopLeft);
        assert_eq!(ui.hints(&view())[2].kind, Kind::Select);
        for _ in 0..3 {
            press(&mut ui, BottomLeft);
        }
        assert_eq!(ui.hints(&view())[2], Hint::new("Effacer…").kind(Kind::Danger));
    }

    #[test]
    fn a_long_press_held_on_the_charging_screen_only_wakes_up() {
        // seen on 2026-10-20: held, the nRF repeats Long every 200 ms; the stream comes back meanwhile, and a
        // repeat on the home screen switched the device off
        let mut ui = Ui::new();
        ui.show_charging();
        let (mut wake_ups, mut others) = (0, 0);
        for t in (0..3_000).step_by(200) {
            let v = View { stream_lost: t < 1_000, plugged: true, ..view() };
            match ui.key(TopLeft, Long, t, &v) {
                Some(Command::WakeUp) => wake_ups += 1,
                Some(_) => others += 1,
                None => {}
            }
        }
        assert_eq!((wake_ups, others), (1, 0));
        assert_eq!(ui.screen(), Screen::Splash);
        ui.end_splash();
        assert_eq!(ui.screen(), Screen::Home);
        // released, pressed long again: switched off
        assert_eq!(ui.key(TopLeft, Long, 4_000, &view()), Some(Command::PowerOff));
        // a click in between ends the hold too
        assert_eq!(ui.key(BottomLeft, Click, 4_100, &view()), None);
        assert_eq!(ui.screen(), Screen::Menu);
    }

    #[test]
    fn a_ride_starts_pauses_resumes_and_ends_confirmed() {
        let mut ui = Ui::new();
        assert_eq!(press(&mut ui, BottomRight), Some(Command::Start));
        assert_eq!(ui.screen(), Screen::Ride);
        assert_eq!(press(&mut ui, BottomRight), Some(Command::Pause));
        assert_eq!(ui.screen(), Screen::Paused);
        assert_eq!(press(&mut ui, BottomRight), Some(Command::Resume));
        assert_eq!(press(&mut ui, BottomRight), Some(Command::Pause));
        // finishing asks first, confirmed with another key, and can be cancelled with either left key
        assert_eq!(press(&mut ui, BottomLeft), None);
        assert_eq!(ui.screen(), Screen::ConfirmStop);
        assert_eq!(press(&mut ui, TopLeft), None);
        assert_eq!(ui.screen(), Screen::Paused);
        press(&mut ui, BottomLeft);
        assert_eq!(press(&mut ui, BottomRight), Some(Command::Stop));
        assert_eq!(ui.screen(), Screen::Saved);
        assert_eq!(press(&mut ui, BottomRight), None);
        assert_eq!(ui.screen(), Screen::Home);
    }

    #[test]
    fn a_finished_ride_is_resumed_from_the_end_screen_or_the_menu() {
        let resumable = View {
            resumable: true,
            ..view()
        };
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, BottomLeft);
        press(&mut ui, BottomRight);
        assert_eq!(ui.screen(), Screen::Saved);
        assert_eq!(
            ui.key(BottomLeft, Click, 0, &resumable),
            Some(Command::ResumeLast)
        );
        assert_eq!(ui.screen(), Screen::Ride);
        // from the home menu
        let mut ui = Ui::new();
        press(&mut ui, BottomLeft);
        assert_eq!(ui.screen(), Screen::Menu);
        assert_eq!(ui.menu(&View { resumable: true, ..view() }).0[0], Some(Item::ResumeLast));
        assert_eq!(
            ui.key(BottomRight, Click, 0, &resumable),
            Some(Command::ResumeLast)
        );
        // nothing to resume: the end screen's bottom left does nothing
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, BottomLeft);
        press(&mut ui, BottomRight);
        assert_eq!(press(&mut ui, BottomLeft), None);
        assert_eq!(ui.hints(&view())[1], NOTHING);
    }

    #[test]
    fn no_key_ends_or_switches_off_a_running_ride() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        for key in [TopLeft, BottomLeft, BottomRight] {
            let c = ui.key(key, Long, 0, &view());
            assert!(c != Some(Command::Stop) && c != Some(Command::PowerOff));
        }
        assert_eq!(ui.screen(), Screen::Ride);
    }

    #[test]
    fn the_bottom_left_key_cycles_the_pages_and_the_top_left_one_scrolls_them() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        assert_eq!(ui.page(), Some(Page::Essential));
        press(&mut ui, BottomLeft);
        assert_eq!(ui.page(), Some(Page::Altitude));
        press(&mut ui, BottomLeft);
        assert_eq!(ui.page(), Some(Page::Essential));
        ui.key(TopLeft, Click, 1_000, &view());
        assert!(ui.scrolling());
        ui.tick(1_000 + SCROLL_MS - 1, Some(true), &view());
        assert_eq!(ui.page(), Some(Page::Essential));
        ui.tick(1_000 + SCROLL_MS, Some(true), &view());
        assert_eq!(ui.page(), Some(Page::Altitude));
        // pausing stops the scrolling
        press(&mut ui, BottomRight);
        assert!(!ui.scrolling());
    }

    #[test]
    fn off_the_route_in_metres_then_in_km_from_2_km() {
        let off = |off_dm| {
            let mut v = view();
            v.route = Some(RouteView {
                name: Line::new(format_args!("x")),
                length_dm: 100_000,
                ascent_m: 0,
                me: Some((0, 0)),
                heading_cdeg: None,
                progress: Some(route::Progress { along_dm: 0, off_dm, segment: 0 }),
            });
            let mut ui = Ui::new();
            press(&mut ui, BottomRight);
            ui.tick(0, Some(true), &v);
            ui.banner().map(|b| Line::new(format_args!("{}", b.detail.as_str())))
        };
        assert_eq!(off(12_340).unwrap().as_str(), "à 1234 m");
        // then how to set the route aside, every other 4 s
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        let mut v = view();
        v.route = Some(RouteView {
            name: Line::new(format_args!("x")),
            length_dm: 0,
            ascent_m: 0,
            me: Some((0, 0)),
            heading_cdeg: None,
            progress: Some(route::Progress { along_dm: 0, off_dm: 12_340, segment: 0 }),
        });
        ui.tick(1_000, Some(true), &v);
        assert_eq!(ui.banner().unwrap().detail.as_str(), "à 1234 m");
        ui.tick(1_000 + OFF_ROUTE_HINT_MS, Some(true), &v);
        assert_eq!(ui.banner().unwrap().detail.as_str(), "Pas cette trace ? Pause › Menu");
        // then only the distance in place of the clock
        ui.tick(1_000 + OFF_ROUTE_ALERT_MS, Some(true), &v);
        assert!(ui.banner().is_none());
        assert_eq!(ui.off_route_dm(), Some(12_340));
        // the banner again when the distance doubles
        let farther = |v: &mut View, off_dm| v.route.as_mut().unwrap().progress.as_mut().unwrap().off_dm = off_dm;
        farther(&mut v, 24_000);
        ui.tick(60_000, Some(true), &v);
        assert!(ui.banner().is_none());
        farther(&mut v, 24_680);
        ui.tick(61_000, Some(true), &v);
        assert_eq!(ui.banner().unwrap().detail.as_str(), "à 2,4 km");
        // back on the route under half the threshold only
        farther(&mut v, OFF_ROUTE_DM / 2);
        ui.tick(62_000, Some(true), &v);
        assert_eq!(ui.off_route_dm(), Some(OFF_ROUTE_DM / 2));
        farther(&mut v, OFF_ROUTE_DM / 2 - 1);
        ui.tick(63_000, Some(true), &v);
        assert_eq!((ui.banner(), ui.off_route_dm()), (None, None));
        assert_eq!(off(23_456).unwrap().as_str(), "à 2,3 km");
        assert!(off(400).is_none());
    }

    #[test]
    fn a_page_changed_by_hand_waits_a_full_interval_before_the_next_scroll() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        ui.key(TopLeft, Click, 0, &view());
        assert!(ui.scrolling());
        // by hand just before the scroll was due
        ui.key(BottomLeft, Click, SCROLL_MS - 100, &view());
        assert_eq!(ui.page(), Some(Page::Altitude));
        ui.tick(SCROLL_MS, Some(true), &view());
        assert_eq!(ui.page(), Some(Page::Altitude));
        ui.tick(2 * SCROLL_MS - 101, Some(true), &view());
        assert_eq!(ui.page(), Some(Page::Altitude));
        ui.tick(2 * SCROLL_MS - 100, Some(true), &view());
        assert_eq!(ui.page(), Some(Page::Essential));
    }

    #[test]
    fn switching_off_from_home_or_the_pause_menu() {
        let mut ui = Ui::new();
        assert_eq!(ui.key(TopLeft, Long, 0, &view()), Some(Command::PowerOff));
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, TopLeft);
        assert_eq!(ui.screen(), Screen::PauseMenu);
        // no route: sensors, power off, status
        assert_eq!(ui.menu(&view()).0[1], Some(Item::PowerOff));
        press(&mut ui, BottomLeft);
        assert_eq!(press(&mut ui, BottomRight), Some(Command::PowerOff));
        // the status from the pause menu goes back to it, then to the pause
        press(&mut ui, BottomLeft);
        press(&mut ui, BottomRight);
        assert_eq!(ui.screen(), Screen::Status);
        press(&mut ui, TopLeft);
        assert_eq!(ui.screen(), Screen::PauseMenu);
        press(&mut ui, TopLeft);
        assert_eq!(ui.screen(), Screen::Paused);
    }

    #[test]
    fn a_route_read_after_the_ride_has_its_map() {
        // a ride resumed at power-on: its pages are picked before the route file is read
        let mut ui = Ui::new();
        ui.found_paused_ride();
        assert_eq!(ui.page(), Some(Page::Essential));
        ui.set_context(true, false, false, Effort::None);
        assert_eq!(ui.page(), Some(Page::Essential), "the page shown stays");
        press(&mut ui, BottomRight); // resume
        let mut pages = [None; 3];
        for p in pages.iter_mut() {
            *p = ui.page();
            press(&mut ui, BottomLeft);
        }
        assert!(pages.contains(&Some(Page::Map)), "{pages:?}");
        // the route removed during the ride: no map page any more
        ui.set_context(false, false, false, Effort::None);
        assert!((0..4).all(|_| {
            press(&mut ui, BottomLeft);
            ui.page() != Some(Page::Map)
        }));
    }

    #[test]
    fn switched_off_after_a_while_still_during_a_ride() {
        let mut ui = Ui::new();
        ui.set_auto_off_min(10);
        let still = View { speed_mm_s: Some(0), ..view() };
        // not without a ride
        assert_eq!(ui.tick(0, None, &still), None);
        assert_eq!(ui.tick(20 * 60_000, None, &still), None);
        press(&mut ui, BottomRight);
        let t0 = 30 * 60_000;
        assert_eq!(ui.tick(t0, Some(true), &still), None);
        // a minute before: announced
        assert_eq!(ui.tick(t0 + 8 * 60_000, Some(true), &still), None);
        assert!(ui.banner().is_none());
        assert_eq!(ui.tick(t0 + 9 * 60_000 + 15_000, Some(true), &still), None);
        let b = ui.banner().unwrap();
        assert_eq!((b.title, b.detail.as_str()), (AUTO_OFF, "Dans 45 s, une touche annule"));
        assert_eq!(ui.tick(t0 + 10 * 60_000, Some(true), &still), Some(Command::PowerOff));
        // paused still counts: the power off pauses the ride and keeps it
        let mut ui = Ui::new();
        ui.set_auto_off_min(10);
        press(&mut ui, BottomRight);
        ui.tick(0, Some(false), &still);
        assert_eq!(ui.tick(10 * 60_000, Some(false), &still), Some(Command::PowerOff));
    }

    #[test]
    fn moving_or_a_key_restarts_the_delay() {
        let mut ui = Ui::new();
        ui.set_auto_off_min(10);
        let still = View { speed_mm_s: None, ..view() };
        press(&mut ui, BottomRight);
        ui.tick(0, Some(true), &still);
        // moving at 9 min: the delay starts again when it stops
        ui.tick(9 * 60_000, Some(true), &View { speed_mm_s: Some(5_000), ..still });
        ui.tick(9 * 60_000 + 1_000, Some(true), &still);
        assert_eq!(ui.tick(18 * 60_000, Some(true), &still), None);
        // pedalling on a home trainer, without a GNSS speed, is moving
        ui.tick(18 * 60_000 + 1, Some(true), &View { cadence_rpm: Some(85), ..still });
        ui.tick(18 * 60_000 + 2, Some(true), &still);
        assert!(ui.banner().is_none_or(|b| b.title != AUTO_OFF));
        // announced, a key cancels it and does nothing else
        let t = 18 * 60_000 + 2 + 9 * 60_000 + 30_000;
        ui.tick(t, Some(true), &still);
        assert_eq!(ui.banner().unwrap().title, AUTO_OFF);
        assert_eq!(ui.key(BottomRight, Click, t + 1, &still), None, "not a pause");
        assert_eq!(ui.screen(), Screen::Ride);
        assert!(ui.banner().is_none_or(|b| b.title != AUTO_OFF));
        assert_eq!(ui.tick(t + 2, Some(true), &still), None);
        assert_eq!(ui.tick(t + 1 + 10 * 60_000, Some(true), &still), Some(Command::PowerOff));
        // off: never
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        assert_eq!(ui.tick(0, Some(true), &still), None);
        assert_eq!(ui.tick(24 * 3_600_000, Some(true), &still), None);
    }

    #[test]
    fn pairing_and_forgetting_a_sensor() {
        let found = |n| {
            Some(Found {
                name: Line::new(format_args!("{n}")),
                rssi_dbm: -60,
                paired: false,
            })
        };
        let mut v = view();
        v.sensors.found = [found("Polar H10"), found("HRM-Pro"), None, None];
        let mut ui = Ui::new();
        press(&mut ui, BottomLeft); // menu: the sensors first without a ride to resume
        assert_eq!(ui.key(BottomRight, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::Sensors);
        // the power meter
        ui.key(BottomLeft, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        assert_eq!(ui.role(), Role::Power);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::Scan(Role::Power)));
        // two found, the role unpaired: no "none" entry, the selection wraps over the two
        ui.key(BottomLeft, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::Pair(Role::Power, 1)));
        assert_eq!(ui.screen(), Screen::Sensors);
        // paired: "none" after the two found
        v.sensors.roles[2].link = Link::Connected;
        ui.key(BottomRight, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::Forget(Role::Power)));
        // paired: its calibration after it, then back to the first role
        assert_eq!(ui.key(BottomLeft, Click, 0, &v), None);
        assert!(ui.calibrate_row());
        assert_eq!(ui.key(BottomRight, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::Calibrate);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::Calibrate));
        v.sensors.calibration = Calibration::Running;
        assert_eq!(ui.key(BottomRight, Click, 0, &v), None, "once at a time");
        assert_eq!(ui.key(TopLeft, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::Sensors);
        ui.key(BottomLeft, Click, 0, &v);
        assert!(!ui.calibrate_row());
        assert_eq!(ui.role(), Role::HeartRate);
        ui.key(BottomLeft, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        // back: the scan stops
        ui.key(BottomRight, Click, 0, &v);
        assert_eq!(ui.key(TopLeft, Click, 0, &v), Some(Command::StopScan));
        assert_eq!(ui.key(TopLeft, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::Menu);
    }

    #[test]
    fn deleting_rides_from_the_menu() {
        let card = RideCard { day: 12, month: 9, timer_s: 3600, ..Default::default() };
        let mut v = View { rides: RidesView { count: 3, card: Some(card), can_start: true }, ..view() };
        let mut ui = Ui::new();
        ui.key(BottomLeft, Click, 0, &v); // menu: sensors, rides, status
        ui.key(BottomLeft, Click, 0, &v);
        assert_eq!(ui.menu(&v).0[ui.item as usize], Some(Item::Rides));
        assert_eq!(ui.key(BottomRight, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::Rides);
        // from the menu, never "start": delete, confirmed
        assert_eq!(ui.hints(&v)[2].text, "Supprimer…");
        ui.key(BottomLeft, Click, 0, &v);
        ui.key(BottomLeft, Click, 0, &v);
        assert_eq!(ui.ride_item(), 2);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::ConfirmDelete);
        assert_eq!(ui.key(TopLeft, Click, 0, &v), None, "cancelled");
        ui.key(BottomRight, Click, 0, &v);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::DeleteRide(2)));
        assert_eq!(ui.screen(), Screen::Rides);
        // the last one deleted: the new last one is selected
        v.rides.count = 2;
        ui.tick(0, None, &v);
        assert_eq!(ui.ride_item(), 1);
        // none left
        v.rides = RidesView { count: 0, card: None, can_start: true };
        ui.tick(0, None, &v);
        assert_eq!(ui.hints(&v)[2], NOTHING);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), None);
        assert_eq!(ui.screen(), Screen::Rides);
        ui.key(TopLeft, Click, 0, &v);
        assert_eq!(ui.screen(), Screen::Menu);
    }

    #[test]
    fn a_ride_refused_for_want_of_room_starts_once_some_is_deleted() {
        let card = RideCard { sent: true, ..Default::default() };
        let mut v = View { rides: RidesView { count: 5, card: Some(card), can_start: false }, ..view() };
        let mut ui = Ui::new();
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::Start));
        ui.make_room();
        assert_eq!((ui.screen(), ui.ride_item(), ui.making_room()), (Screen::Rides, 0, true));
        ui.key(BottomRight, Click, 0, &v);
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::DeleteRide(0)));
        v.rides.count = 4;
        v.rides.can_start = true;
        assert_eq!(ui.hints(&v)[2].text, "Démarrer");
        assert_eq!(ui.key(BottomRight, Click, 0, &v), Some(Command::Start));
        assert_eq!(ui.screen(), Screen::Ride);
        // back from the screen opened by a refusal: home
        ui.make_room();
        ui.key(TopLeft, Click, 0, &v);
        assert_eq!(ui.screen(), Screen::Home);
    }

    #[test]
    fn a_paused_ride_found_at_power_on() {
        let mut ui = Ui::new();
        ui.found_paused_ride();
        ui.tick(0, Some(false), &view());
        assert_eq!(ui.screen(), Screen::Resume);
        press(&mut ui, BottomLeft);
        assert_eq!(press(&mut ui, TopLeft), None);
        assert_eq!(ui.screen(), Screen::Resume);
        assert_eq!(press(&mut ui, BottomRight), Some(Command::Resume));
        assert_eq!(ui.page(), Some(Page::Essential));
    }

    #[test]
    fn the_screen_follows_the_ride_the_firmware_has() {
        let mut ui = Ui::new();
        // start refused (no time yet)
        press(&mut ui, BottomRight);
        ui.tick(0, None, &view());
        assert_eq!(ui.screen(), Screen::Home);
        // pause refused: the timer still runs
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        ui.tick(0, Some(true), &view());
        assert_eq!(ui.screen(), Screen::Ride);
    }

    #[test]
    fn gnss_banners() {
        let searching = View {
            gnss: Gnss::Searching {
                in_view: 5,
                progress: 40,
            },
            ..Default::default()
        };
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        ui.tick(0, Some(true), &searching);
        assert_eq!(ui.banner().unwrap().title, "Recherche du GPS");
        assert!(ui.gps_warning());
        ui.tick(100, Some(true), &view());
        assert_eq!(ui.banner(), None, "first fix: no banner");
        ui.tick(200, Some(true), &searching);
        assert_eq!(ui.banner().unwrap().title, "GPS perdu");
        ui.tick(300, Some(true), &view());
        let b = ui.banner().unwrap();
        assert_eq!(
            (b.kind, b.title, b.detail.as_str()),
            (BannerKind::Info, "GPS retrouvé", "9 satellites")
        );
        ui.tick(300 + INFO_MS, Some(true), &view());
        assert_eq!(ui.banner(), None);
    }

    #[test]
    fn a_progress_shows_what_is_done_while_it_moves() {
        let mut ui = Ui::new();
        let pct = |p: u32| Line::new(format_args!("{p} %"));
        ui.progress("Réception de la trace", pct(25), 2_500, 10_000, 0);
        ui.tick(4_000, Some(true), &view());
        let b = ui.banner().unwrap();
        assert_eq!((b.kind, b.bar_permille), (BannerKind::Progress, 250), "no clock on the bar");
        // news within PROGRESS_MS: it stays, longer than an information would
        ui.progress("Réception de la trace", pct(70), 7_000, 10_000, 4_000);
        ui.tick(4_000 + PROGRESS_MS - 1, Some(true), &view());
        assert_eq!(ui.banner().unwrap().bar_permille, 700);
        // the end says so, with no bar: it goes after INFO_MS without a sign
        ui.inform("Trace reçue", Line::new(format_args!("Col")), 9_000);
        let b = ui.banner().unwrap();
        assert_eq!((b.kind, b.bar_permille), (BannerKind::Info, 0));
        ui.tick(9_000 + INFO_MS - 1, Some(true), &view());
        assert_eq!(ui.banner().unwrap().bar_permille, 0);
        // no news: a transfer cut short does not stay on the screen
        ui.progress("Réception de la carte", pct(10), 1, 10, 10_000);
        ui.tick(10_000 + PROGRESS_MS, Some(true), &view());
        assert_eq!(ui.banner(), None);
    }

    #[test]
    fn a_ride_no_longer_recorded_says_so_first() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        let full = View {
            write_failure: Some(WriteFailure::Full),
            gnss: Gnss::Searching { in_view: 0, progress: 0 },
            ..view()
        };
        ui.tick(0, Some(true), &full);
        let b = ui.banner().unwrap();
        assert_eq!(
            (b.kind, b.title, b.detail.as_str()),
            (BannerKind::Attention, "Mémoire pleine", "La sortie n'est plus enregistrée")
        );
        ui.tick(100, Some(true), &View { write_failure: Some(WriteFailure::Error), ..view() });
        assert_eq!(ui.banner().unwrap().title, "Erreur d'écriture");
        ui.tick(200, Some(true), &view());
        let b = ui.banner().unwrap();
        assert_eq!((b.kind, b.title), (BannerKind::Info, "Enregistrement repris"));
    }

    #[test]
    fn page_names() {
        // route, climbs, train, a power meter
        let names = |r, c, tr, e: bool| pages(r, c, tr, if e { Effort::Power } else { Effort::None }).names();
        assert_eq!(names(false, false, false, false).as_str(), "Essentiel · Altitude");
        assert_eq!(names(true, false, false, false).as_str(), "Carte · Essentiel · Altitude");
        assert_eq!(names(true, true, false, false).as_str(), "Carte · Essentiel · Montées");
        assert_eq!(names(true, false, true, false).as_str(), "Essentiel · Carte · Altitude");
        assert_eq!(names(true, true, true, false).as_str(), "Essentiel · Carte · Montées");
        assert_eq!(names(false, false, false, true).as_str(), "Essentiel · Altitude · Effort");
        assert_eq!(names(true, false, false, true).as_str(), "Carte · Essentiel · Altitude · Effort");
        assert_eq!(names(true, true, false, true).as_str(), "Carte · Essentiel · Montées · Effort");
        assert_eq!(names(true, true, true, true).as_str(), "Effort · Essentiel · Carte · Montées");
        assert_eq!(names(false, false, true, true).as_str(), "Effort · Essentiel · Altitude");
        // climbs without a route followed (set aside): Altitude
        assert_eq!(names(false, true, false, false).as_str(), "Essentiel · Altitude");
        // a heart rate sensor without a power meter: Cardio in Effort's place
        assert_eq!(pages(false, false, false, Effort::Heart).names().as_str(), "Essentiel · Altitude · Cardio");
        assert_eq!(pages(true, false, true, Effort::Heart).names().as_str(), "Cardio · Essentiel · Carte · Altitude");
    }

    #[test]
    fn the_effort_page_follows_the_sensors_connected() {
        let row = |link| SensorRow { link, name: None };
        // heart rate, speed / cadence, power
        let s = |hr, power| Sensors { roles: [row(hr), row(Link::Unpaired), row(power)], ..Default::default() };
        use Link::*;
        let mut pick = EffortPick::default();
        // before the start: what is connected now
        assert_eq!(pick.update(false, &s(Connected, Searching), 0), Effort::Heart, "the power meter is off");
        assert_eq!(pick.update(false, &s(Connected, Connected), 0), Effort::Power);
        assert_eq!(pick.update(false, &s(Searching, Searching), 0), Effort::None);
        // started with the power meter off: Cardio, then Effort when it connects in the first minutes
        let mut pick = EffortPick::default();
        assert_eq!(pick.update(true, &s(Connected, Searching), 1_000), Effort::Heart);
        assert_eq!(pick.update(true, &s(Connected, Connected), 60_000), Effort::Power);
        // lost during the ride: Effort stays
        assert_eq!(pick.update(true, &s(Connected, Searching), 120_000), Effort::Power);
        // too late: Cardio stays
        let mut pick = EffortPick::default();
        pick.update(true, &s(Connected, Searching), 0);
        assert_eq!(pick.update(true, &s(Connected, Connected), EFFORT_GRACE_MS), Effort::Heart);
        // ... unless the power meter was paired during the ride
        let mut pick = EffortPick::default();
        pick.update(true, &s(Connected, Unpaired), 0);
        pick.update(true, &s(Connected, Searching), EFFORT_GRACE_MS * 2);
        assert_eq!(pick.update(true, &s(Connected, Connected), EFFORT_GRACE_MS * 2 + 5_000), Effort::Power);
        // forgotten: its page goes
        assert_eq!(pick.update(true, &s(Connected, Unpaired), EFFORT_GRACE_MS * 3), Effort::Heart);
        assert_eq!(pick.update(true, &s(Unpaired, Unpaired), EFFORT_GRACE_MS * 3), Effort::None);
    }

    #[test]
    fn a_zone_shows_once_the_value_stayed_in_it() {
        let zones = Zones::parse(settings::POWER_ZONES).unwrap();
        let v = |w| View { power_3s_w: Some(w), ftp_w: 230, power_zones: zones, ..view() };
        let mut ui = Ui::new();
        ui.tick(0, Some(true), &v(215));
        assert_eq!(ui.power_zone(), Some(4), "at once from no zone");
        // over 241 W for less than the hold: still Z4
        ui.tick(1_000, Some(true), &v(245));
        ui.tick(3_000, Some(true), &v(250));
        assert_eq!(ui.power_zone(), Some(4));
        // back in Z4 restarts the hold
        ui.tick(3_500, Some(true), &v(230));
        ui.tick(4_000, Some(true), &v(250));
        ui.tick(6_500, Some(true), &v(250));
        assert_eq!(ui.power_zone(), Some(4));
        ui.tick(7_000, Some(true), &v(250));
        assert_eq!(ui.power_zone(), Some(5));
        // no power, or no FTP: no zone
        ui.tick(7_100, Some(true), &View { power_3s_w: None, ..v(0) });
        assert_eq!(ui.power_zone(), None);
        ui.tick(7_200, Some(true), &View { ftp_w: 0, ..v(215) });
        assert_eq!(ui.power_zone(), None);
        assert_eq!(ui.hr_zone(), None, "no heart rate");
    }

    #[test]
    fn a_route_with_climbs_swaps_altitude_for_climbs_during_the_ride() {
        let mut ui = Ui::new();
        ui.found_paused_ride();
        ui.set_context(true, false, false, Effort::None);
        assert_eq!(ui.next_pages().names().as_str(), "Carte · Essentiel · Altitude");
        ui.set_context(true, true, false, Effort::None);
        assert_eq!(ui.next_pages().names().as_str(), "Carte · Essentiel · Montées");
    }

    #[test]
    fn a_power_meter_paired_during_the_ride_brings_effort() {
        let mut ui = Ui::new();
        ui.found_paused_ride();
        ui.set_context(false, false, false, Effort::Power);
        assert_eq!(ui.page(), Some(Page::Essential), "the page shown stays");
        assert_eq!(ui.next_pages().names().as_str(), "Essentiel · Altitude · Effort");
    }

    #[test]
    fn auto_pause_keeps_ride_pages_and_allows_a_manual_pause() {
        let mut ui = Ui::new();
        ui.end_splash();
        let mut v = View { gnss: Gnss::Fix { used: 8 }, ..Default::default() };
        v.ride.auto_paused = true;
        ui.tick(0, Some(true), &v);
        assert_eq!(ui.screen(), Screen::Ride);
        assert_eq!(ui.banner().unwrap().title, "Pause automatique");
        assert_eq!(ui.key(Key::BottomRight, Press::Click, 1, &v), Some(Command::Pause));
        v.ride.auto_paused = false;
        ui.tick(2, Some(false), &v);
        assert_eq!(ui.screen(), Screen::Paused);
        assert!(ui.banner().is_none());
        assert_eq!(ui.key(Key::BottomRight, Press::Click, 3, &v), Some(Command::Resume));
        ui.tick(4, Some(true), &v);
        assert_eq!(ui.screen(), Screen::Ride);
    }

    #[test]
    fn average_speed() {
        let r = RideView {
            timer_s: 3600,
            elapsed_s: 7200,
            auto_paused: true,
            distance_cm: 2_850_000,
            ..Default::default()
        };
        // 28.5 km in an hour
        assert_eq!(r.avg_speed_mm_s(), Some(7916));
        assert_eq!(RideView::default().avg_speed_mm_s(), None);
    }
}
