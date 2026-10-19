//! Ride UI of the CC600, hardware independent: the screens, what the 3 keys do on each, and their drawing on the
//! 240x320 panel ([`draw`]). The design follows the clickable prototype (docs/prototype), in French.
//!
//! The firmware owns the ride: [`Ui::key`] only returns a [`Command`] for it to run, and [`Ui::tick`] brings the
//! screen back in line with the ride it really has (a start refused without the time, a ride lost).
//!
//! Keys (nRF numbering): 0 top left, 1 bottom left, 2 bottom right. The bottom right key is the main action (start,
//! pause, resume); a ride ends from the pause screen, confirmed, never by one press during the ride.

#![no_std]

mod draw;

pub use draw::{CONTENT, DAY, NIGHT, Theme, draw};

/// Ride pages, cycled by the two left keys: 0 speed / time / distance / D+ / heart rate, 1 altitude / averages.
pub const PAGES: u8 = 2;
/// The power-off prompt closes by itself after this long.
pub const POWER_PROMPT_MS: u64 = 8_000;
/// Notice when a ride cannot start yet: no GNSS fix and no RTC time.
pub const NOTICE_NO_TIME: &str = "Pas encore l'heure";
/// A notice stays this long on the screen.
pub const NOTICE_MS: u64 = 3_000;

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
    /// Bring-up values (sensors, links, storage), drawn by the firmware in [`CONTENT`].
    Status,
    Ride,
    Paused,
    ConfirmStop,
    Saved,
    PowerOff,
}

/// What the firmware has to do after a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    Pause,
    Resume,
    /// Stop and save the ride.
    Stop,
    /// Power off; a ride in progress is saved first.
    PowerOff,
}

/// Colour of a key label: the main actions have their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Go,
    Pause,
    Danger,
    Select,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Power,
    Left,
    Right,
}

/// What a key does now, written next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub text: &'static str,
    pub before: Option<Icon>,
    pub after: Option<Icon>,
    pub kind: Kind,
}

impl Hint {
    const fn new(text: &'static str) -> Self {
        Self {
            text,
            before: None,
            after: None,
            kind: Kind::Plain,
        }
    }

    const fn before(mut self, icon: Icon) -> Self {
        self.before = Some(icon);
        self
    }

    const fn after(mut self, icon: Icon) -> Self {
        self.after = Some(icon);
        self
    }

    const fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
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
    pub heart_rate: Option<u8>,
    /// The ride in progress, or the last one saved (for [`Screen::Saved`]).
    pub ride: RideView,
    pub rides_stored: Option<u16>,
    /// Recording time left in the storage.
    pub free_hours: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gnss {
    #[default]
    Off,
    Searching {
        in_view: u8,
    },
    Fix {
        used: u8,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RideView {
    pub timer_s: u32,
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

/// Which screen is shown and what the keys do on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    screen: Screen,
    page: u8,
    /// Where the power-off prompt goes back to.
    back: Screen,
    prompt_at: u64,
    notice: Option<(&'static str, u64)>,
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
            page: 0,
            back: Screen::Home,
            prompt_at: 0,
            notice: None,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn page(&self) -> u8 {
        self.page
    }

    pub fn notice(&self) -> Option<&'static str> {
        self.notice.map(|(text, _)| text)
    }

    /// Shows `text` for [`NOTICE_MS`].
    pub fn set_notice(&mut self, text: &'static str, now_ms: u64) {
        self.notice = Some((text, now_ms));
    }

    pub fn key(&mut self, key: Key, press: Press, now_ms: u64) -> Option<Command> {
        use Key::*;
        use Press::*;
        use Screen::*;
        let (screen, command) = match (self.screen, key, press) {
            (Home | Paused, TopLeft, Long) => {
                self.back = self.screen;
                self.prompt_at = now_ms;
                (PowerOff, None)
            }
            (PowerOff, TopLeft, Click) => (PowerOff, Some(Command::PowerOff)),
            (PowerOff, _, Click) => (self.back, None),
            (Home, BottomLeft, Click) => (Status, None),
            (Home, BottomRight, Click) => {
                self.page = 0;
                (Ride, Some(Command::Start))
            }
            (Status, TopLeft, Click) => (Home, None),
            (Ride, TopLeft, Click) => {
                self.page = (self.page + PAGES - 1) % PAGES;
                (Ride, None)
            }
            (Ride, BottomLeft, Click) => {
                self.page = (self.page + 1) % PAGES;
                (Ride, None)
            }
            (Ride, BottomRight, Click) => (Paused, Some(Command::Pause)),
            (Paused, BottomLeft, Click) => (ConfirmStop, None),
            (Paused, BottomRight, Click) => (Ride, Some(Command::Resume)),
            (ConfirmStop, TopLeft | BottomLeft, Click) => (Paused, None),
            (ConfirmStop, BottomRight, Click) => (Saved, Some(Command::Stop)),
            (Saved, BottomRight, Click) => (Home, None),
            (s, _, _) => (s, None),
        };
        self.screen = screen;
        command
    }

    /// Closes what times out, and follows the ride the firmware has: `None` without a ride, else whether its timer
    /// runs.
    pub fn tick(&mut self, now_ms: u64, ride: Option<bool>) {
        use Screen::*;
        if self.screen == PowerOff && now_ms.saturating_sub(self.prompt_at) >= POWER_PROMPT_MS {
            self.screen = self.back;
        }
        if self
            .notice
            .is_some_and(|(_, at)| now_ms.saturating_sub(at) >= NOTICE_MS)
        {
            self.notice = None;
        }
        self.screen = match (self.screen, ride) {
            (Ride | Paused | ConfirmStop, None) => Home,
            (Home | Status | Saved | Ride, Some(false)) => Paused,
            (Home | Status | Saved | Paused | ConfirmStop, Some(true)) => Ride,
            (s, _) => s,
        };
        if self.back == Paused && ride.is_none() {
            self.back = Home;
        }
    }

    /// Labels of the top left, bottom left and bottom right keys.
    pub fn hints(&self) -> [Option<Hint>; 3] {
        use Screen::*;
        const HOLD_OFF: Hint = Hint::new("Maintenir").before(Icon::Power);
        const BACK: Hint = Hint::new("Retour").before(Icon::Left);
        match self.screen {
            Home => [
                Some(HOLD_OFF),
                Some(Hint::new("État")),
                Some(Hint::new("Démarrer").kind(Kind::Go)),
            ],
            Status => [Some(BACK), None, None],
            Ride => [
                Some(Hint::new("Page").before(Icon::Left)),
                Some(Hint::new("Page").after(Icon::Right)),
                Some(Hint::new("Pause").after(Icon::Pause).kind(Kind::Pause)),
            ],
            Paused => [
                Some(HOLD_OFF),
                Some(Hint::new("Terminer")),
                Some(Hint::new("Reprendre").kind(Kind::Go)),
            ],
            ConfirmStop => [
                Some(Hint::new("Annuler").before(Icon::Left)),
                Some(Hint::new("Annuler")),
                Some(Hint::new("Terminer").kind(Kind::Danger)),
            ],
            Saved => [None, None, Some(Hint::new("OK").kind(Kind::Select))],
            PowerOff => [
                Some(Hint::new("Oui").before(Icon::Power).kind(Kind::Danger)),
                Some(Hint::new("Non")),
                Some(Hint::new("Non")),
            ],
        }
    }

    /// Whether the power-off prompt would save a ride.
    pub fn power_off_saves_ride(&self) -> bool {
        self.screen == Screen::PowerOff && self.back == Screen::Paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::*;
    use Press::*;

    fn press(ui: &mut Ui, key: Key) -> Option<Command> {
        ui.key(key, Click, 0)
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
        // ending asks first, and can be cancelled with either left key
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
    fn no_key_ends_or_powers_off_a_running_ride() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        for key in [TopLeft, BottomLeft] {
            for p in [Click, Long] {
                assert_eq!(ui.key(key, p, 0), None);
                assert_eq!(ui.screen(), Screen::Ride);
            }
        }
        assert_eq!(ui.key(BottomRight, Long, 0), None);
        assert_eq!(ui.screen(), Screen::Ride);
    }

    #[test]
    fn the_left_keys_cycle_the_ride_pages() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        assert_eq!(ui.page(), 0);
        press(&mut ui, BottomLeft);
        assert_eq!(ui.page(), 1);
        press(&mut ui, BottomLeft);
        assert_eq!(ui.page(), 0);
        press(&mut ui, TopLeft);
        assert_eq!(ui.page(), PAGES - 1);
        // a new ride starts on the first page
        press(&mut ui, BottomRight);
        press(&mut ui, BottomLeft);
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        assert_eq!(ui.page(), 0);
    }

    #[test]
    fn power_off_asks_and_closes_by_itself() {
        let mut ui = Ui::new();
        assert_eq!(ui.key(TopLeft, Long, 1_000), None);
        assert_eq!(ui.screen(), Screen::PowerOff);
        // the nRF repeats the long press while the key is held
        assert_eq!(ui.key(TopLeft, Long, 1_500), None);
        assert_eq!(ui.screen(), Screen::PowerOff);
        assert!(!ui.power_off_saves_ride());
        ui.tick(1_000 + POWER_PROMPT_MS - 1, None);
        assert_eq!(ui.screen(), Screen::PowerOff);
        ui.tick(1_000 + POWER_PROMPT_MS, None);
        assert_eq!(ui.screen(), Screen::Home);

        ui.key(TopLeft, Long, 0);
        assert_eq!(press(&mut ui, BottomRight), None);
        assert_eq!(ui.screen(), Screen::Home);
        ui.key(TopLeft, Long, 0);
        assert_eq!(press(&mut ui, TopLeft), Some(Command::PowerOff));
    }

    #[test]
    fn power_off_from_the_pause_saves_the_ride() {
        let mut ui = Ui::new();
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        ui.key(TopLeft, Long, 0);
        assert!(ui.power_off_saves_ride());
        press(&mut ui, BottomLeft);
        assert_eq!(ui.screen(), Screen::Paused);
    }

    #[test]
    fn the_screen_follows_the_ride_the_firmware_has() {
        let mut ui = Ui::new();
        // start refused (no time yet)
        press(&mut ui, BottomRight);
        ui.tick(0, None);
        assert_eq!(ui.screen(), Screen::Home);
        // pause refused: the timer still runs
        press(&mut ui, BottomRight);
        press(&mut ui, BottomRight);
        ui.tick(0, Some(true));
        assert_eq!(ui.screen(), Screen::Ride);
        // the status screen never hides a ride
        let mut ui = Ui::new();
        press(&mut ui, BottomLeft);
        assert_eq!(ui.screen(), Screen::Status);
        ui.tick(0, Some(false));
        assert_eq!(ui.screen(), Screen::Paused);
    }

    #[test]
    fn notices_time_out() {
        let mut ui = Ui::new();
        ui.set_notice("Pas encore l'heure", 100);
        ui.tick(100 + NOTICE_MS - 1, None);
        assert_eq!(ui.notice(), Some("Pas encore l'heure"));
        ui.tick(100 + NOTICE_MS, None);
        assert_eq!(ui.notice(), None);
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
