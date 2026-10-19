//! Ride UI of the CC600, hardware independent: the screens, what the 3 keys do on each, and their drawing on the
//! 240x320 panel ([`draw`]). The design follows the clickable prototype (docs/prototype), in French.
//!
//! The firmware owns the ride: [`Ui::key`] only returns a [`Command`] for it to run, and [`Ui::tick`] brings the
//! screen back in line with the ride it really has (a start refused without the time, a ride lost).
//!
//! Keys (nRF numbering): 0 top left, 1 bottom left, 2 bottom right. Bottom right is the main action (start, pause,
//! resume, OK), bottom left the menu or the next page / item, top left back or contextual. One long press only:
//! the top left key on the home screen switches off. A ride ends from the pause screen, confirmed with another
//! key, and stays resumable afterwards.

#![no_std]

mod draw;
pub mod fonts;
pub mod history;

pub use draw::{CONTENT, DAY, NIGHT, Theme, draw};
use fonts::Line;

/// Pages of a ride at most.
pub const MAX_PAGES: usize = 4;
/// Auto-scroll of the ride pages, one page every this long (default of [`Ui::set_scroll_s`]).
pub const SCROLL_MS: u64 = 4_000;
/// An information banner stays this long.
pub const INFO_MS: u64 = 3_000;
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

/// The nRF repeats `Long` while the key is held: a long press does nothing more on the screen it opens.
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
    Ride,
    Paused,
    PauseMenu,
    /// A paused ride found at power-on.
    Resume,
    ConfirmStop,
    Saved,
}

/// A ride page, picked when the ride starts ([`pages`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Speed, time, distance, D+, heart rate (else average speed).
    Essential,
    /// Altitude, gradient, D+ / D-, the last 5 km.
    Altitude,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Essential => "Essentiel",
            Page::Altitude => "Altitude",
        }
    }
}

/// The ride pages for what the device has (docs/PRODUCT.md, "Data pages"): without a route nor a sensor,
/// Essential then Altitude. The map, climbs and effort pages come with the route and the sensors.
pub fn pages() -> Pages {
    Pages {
        list: [Some(Page::Essential), Some(Page::Altitude), None, None],
    }
}

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
    /// Resume the last finished ride (reopened, timer running).
    ResumeLast,
    /// Switch off; a paused ride is kept and offered again at power-on.
    PowerOff,
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
    /// When it was shown (an information goes after [`INFO_MS`]).
    pub since_ms: u64,
    /// For an information: what is left of its time, in tenths (the line under it).
    pub left_tenths: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    /// Blue, for [`INFO_MS`].
    Info,
    /// Amber, while it is true.
    Attention,
}

/// Entries of the menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    ResumeLast,
    Status,
    PowerOff,
}

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::ResumeLast => "\u{e00d} Reprendre la sortie",
            Item::Status => "État du compteur",
            Item::PowerOff => "Éteindre (sortie gardée)",
        }
    }
}

/// What the screens show, filled by the firmware before each [`draw`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    /// Local time, hours and minutes.
    pub clock: Option<(u8, u8)>,
    pub battery_pct: Option<u8>,
    pub gnss: Gnss,
    /// Current speed, 0 when stopped.
    pub speed_mm_s: Option<u32>,
    pub altitude_dm: Option<i32>,
    /// Gradient over the last 100 m, percent.
    pub grade_pct: Option<i8>,
    pub heart_rate: Option<u8>,
    /// The ride in progress, or the last one finished (for [`Screen::Saved`]).
    pub ride: RideView,
    /// Altitude (dm) of the last 5 km, every 100 m, oldest first.
    pub recent: Profile,
    /// Altitude (dm) of the whole ride.
    pub whole: Profile,
    /// Finished rides on the device (all still to upload).
    pub rides_stored: Option<u16>,
    /// The last finished ride can be resumed.
    pub resumable: bool,
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
    pub timer_s: u32,
    pub elapsed_s: u32,
    pub distance_cm: u32,
    pub ascent_m: u16,
    pub descent_m: u16,
    pub max_speed_mm_s: u32,
}

impl RideView {
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

/// Which screen is shown and what the keys do on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    screen: Screen,
    /// Where the menus, the status and the confirmation go back to.
    back: Screen,
    pages: Pages,
    page: u8,
    item: u8,
    scroll: bool,
    scroll_ms: u64,
    scrolled_at: u64,
    banner: Option<Banner>,
    /// The GNSS had a fix during this ride ("lost" rather than "searching").
    had_fix: bool,
    fix: bool,
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
            scroll: false,
            scroll_ms: SCROLL_MS,
            scrolled_at: 0,
            banner: None,
            had_fix: false,
            fix: false,
        }
    }

    /// Auto-scroll interval (the `scroll_s` setting).
    pub fn set_scroll_s(&mut self, s: u8) {
        self.scroll_ms = s as u64 * 1000;
    }

    /// At power-on with a paused ride: the screen that offers to resume or finish it.
    pub fn found_paused_ride(&mut self) {
        self.screen = Screen::Resume;
        self.pages = pages();
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn page(&self) -> Option<Page> {
        self.pages.get(self.page as usize)
    }

    /// The page shown and how many there are.
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
        if !self.banner.is_some_and(|b| b.kind == BannerKind::Attention) {
            self.banner = Some(Banner {
                kind: BannerKind::Info,
                title,
                detail,
                since_ms: now_ms,
                left_tenths: 10,
            });
        }
    }

    /// The entries of the menu shown, and the selected one.
    pub fn menu(&self, resumable: bool) -> ([Option<Item>; 3], u8) {
        match self.screen {
            Screen::Menu if resumable => (
                [Some(Item::ResumeLast), Some(Item::Status), None],
                self.item,
            ),
            Screen::Menu => ([Some(Item::Status), None, None], self.item),
            Screen::PauseMenu => ([Some(Item::PowerOff), Some(Item::Status), None], self.item),
            _ => ([None; 3], 0),
        }
    }

    pub fn key(&mut self, key: Key, press: Press, now_ms: u64, view: &View) -> Option<Command> {
        use Key::*;
        use Press::*;
        use Screen::*;
        let (screen, command) = match (self.screen, key, press) {
            (Home, TopLeft, Long) => (Home, Some(Command::PowerOff)),
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
                let n = self.menu(view.resumable).0.iter().flatten().count() as u8;
                self.item = (self.item + 1) % n.max(1);
                (self.screen, None)
            }
            (Menu | PauseMenu, BottomRight, Click) => {
                match self.menu(view.resumable).0[self.item as usize] {
                    Some(Item::ResumeLast) => {
                        self.start_ride();
                        (Ride, Some(Command::ResumeLast))
                    }
                    // the status goes back to its menu, the menu to where it was opened from
                    Some(Item::Status) => (Status, None),
                    Some(Item::PowerOff) => (self.screen, Some(Command::PowerOff)),
                    None => (self.screen, None),
                }
            }
            (Status, TopLeft, Click) => (if self.back == Home { Menu } else { PauseMenu }, None),
            (Ride, TopLeft, Click) => {
                self.scroll = !self.scroll;
                self.scrolled_at = now_ms;
                (Ride, None)
            }
            (Ride, BottomLeft, Click) => {
                self.next_page();
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
                (ConfirmStop, None)
            }
            (Paused | Resume, BottomRight, Click) => (Ride, Some(Command::Resume)),
            (ConfirmStop, TopLeft | BottomLeft, Click) => (self.back, None),
            (ConfirmStop, BottomRight, Click) => (Saved, Some(Command::Stop)),
            (Saved, BottomLeft, Click) if view.resumable => {
                self.start_ride();
                (Ride, Some(Command::ResumeLast))
            }
            (Saved, BottomRight, Click) => (Home, None),
            (s, _, _) => (s, None),
        };
        self.screen = screen;
        command
    }

    fn start_ride(&mut self) {
        self.pages = pages();
        self.page = 0;
        self.had_fix = false;
    }

    fn next_page(&mut self) {
        self.page = (self.page + 1) % (self.pages.len() as u8).max(1);
    }

    /// Follows the ride the firmware has (`None` without a ride, else whether its timer runs) and the GNSS fix;
    /// scrolls the pages, sets and clears the banners.
    pub fn tick(&mut self, now_ms: u64, ride: Option<bool>, view: &View) {
        use Screen::*;
        self.screen = match (self.screen, ride) {
            (Ride | Paused | PauseMenu | ConfirmStop | Resume, None) => Home,
            (Home | Saved | Paused | ConfirmStop | Resume, Some(true)) => Ride,
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
        // GNSS banners while riding: searching or lost while there is no fix, then found again for a while
        let fix = matches!(view.gnss, Gnss::Fix { .. });
        let riding = ride.is_some();
        if riding && !fix {
            let (title, detail) = if self.had_fix {
                ("GPS perdu", "Distance en attente du GPS")
            } else {
                ("Recherche du GPS", "La distance part au fix")
            };
            if !self.banner.is_some_and(|b| b.title == title) {
                self.banner = Some(Banner {
                    kind: BannerKind::Attention,
                    title,
                    detail: Line::new(format_args!("{detail}")),
                    since_ms: now_ms,
                    left_tenths: 10,
                });
            }
        } else if self.banner.is_some_and(|b| b.kind == BannerKind::Attention) {
            self.banner = None;
            if let (true, Gnss::Fix { used }) = (riding && self.had_fix, view.gnss) {
                self.inform(
                    "GPS retrouvé",
                    Line::new(format_args!("{used} satellites")),
                    now_ms,
                );
            }
        }
        if riding && fix {
            self.had_fix = true;
        }
        self.fix = fix;
        if let Some(b) = self.banner.as_mut().filter(|b| b.kind == BannerKind::Info) {
            let age = now_ms.saturating_sub(b.since_ms);
            if age >= INFO_MS {
                self.banner = None;
            } else {
                b.left_tenths = (10 - age * 10 / INFO_MS) as u8;
            }
        }
    }

    /// Labels of the top left, bottom left and bottom right keys.
    pub fn hints(&self, view: &View) -> [Hint; 3] {
        use Screen::*;
        const BACK: Hint = Hint::new("\u{e003} Retour");
        const NEXT: Hint = Hint::new("Suivant \u{e00a}");
        const OPEN: Hint = Hint::new("Ouvrir \u{e00b}").kind(Kind::Select);
        const MENU: Hint = Hint::new("\u{e009} Menu");
        match self.screen {
            Home => [
                Hint::new("\u{e005} Maintenir"),
                MENU,
                Hint::new("Démarrer").kind(Kind::Go),
            ],
            Menu | PauseMenu => [BACK, NEXT, OPEN],
            Status => [BACK, NOTHING, NOTHING],
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

    /// The top bar shows "GPS ✕" in place of the clock (riding without a fix).
    pub fn gps_warning(&self) -> bool {
        matches!(self.screen, Screen::Ride | Screen::Paused) && !self.fix
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
            ..Default::default()
        }
    }

    fn press(ui: &mut Ui, key: Key) -> Option<Command> {
        ui.key(key, Click, 0, &view())
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
        assert_eq!(ui.menu(true).0[0], Some(Item::ResumeLast));
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
    fn switching_off_from_home_or_the_pause_menu() {
        let mut ui = Ui::new();
        assert_eq!(ui.key(TopLeft, Long, 0, &view()), Some(Command::PowerOff));
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, TopLeft);
        assert_eq!(ui.screen(), Screen::PauseMenu);
        assert_eq!(ui.menu(false).0[0], Some(Item::PowerOff));
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
    fn page_names() {
        assert_eq!(pages().names().as_str(), "Essentiel · Altitude");
    }

    #[test]
    fn average_speed() {
        let r = RideView {
            timer_s: 3600,
            distance_cm: 2_850_000,
            ..Default::default()
        };
        // 28.5 km in an hour
        assert_eq!(r.avg_speed_mm_s(), Some(7916));
        assert_eq!(RideView::default().avg_speed_mm_s(), None);
    }
}
