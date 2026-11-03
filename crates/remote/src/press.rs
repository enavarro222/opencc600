//! A remote's reports, back to its button presses. Cheap remotes ("TikTok" remotes, page turners) fake a
//! touchscreen: a button is a finger stroke (a swipe, a tap at a fixed spot), sometimes with a key (volume) at the
//! same time; others send keys only. A press is everything from the first touch or key down to the moment nothing is
//! down for [`QUIET_MS`].

use crate::hid::{Code, Layout};

/// Nothing down this long ends a press. A button's key and stroke overlap (the photo button of a BLE-M3: the key
/// up 24 ms before the finger); two presses of a hurried rider were 70 ms apart.
pub const QUIET_MS: u64 = 40;

/// Coordinates of a stroke, the touch area scaled to 0..=[`SCALE`] on both axes.
pub const SCALE: u16 = 1000;

/// How far two strokes' ends may be to be the same button, in [`SCALE`] units (each axis). A button repeats its
/// stroke to the unit; the closest two buttons seen are two taps of a Yiser-J6, 120 apart.
pub const TOLERANCE: u16 = 40;

/// What a button sends, as far as telling the buttons apart goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    /// A key, the first one down (some buttons also make a stroke: the key is what tells them apart, the stroke
    /// may be the one of another button).
    Key(Code),
    /// A finger stroke: where it touched, where it left (the same for a tap).
    Stroke { from: (u16, u16), to: (u16, u16) },
}

impl Signature {
    /// The same button.
    pub fn matches(&self, other: &Signature) -> bool {
        let near = |a: (u16, u16), b: (u16, u16)| a.0.abs_diff(b.0) <= TOLERANCE && a.1.abs_diff(b.1) <= TOLERANCE;
        match (self, other) {
            (Signature::Key(a), Signature::Key(b)) => a == b,
            (Signature::Stroke { from: f1, to: t1 }, Signature::Stroke { from: f2, to: t2 }) => {
                near(*f1, *f2) && near(*t1, *t2)
            }
            _ => false,
        }
    }
}

/// Report IDs whose keys are followed (a remote has one or two).
const REPORTS: usize = 8;

/// Presses rebuilt from one remote's reports.
#[derive(Clone, Copy, Debug)]
pub struct Presses {
    layout: Layout,
    finger: Option<Finger>,
    /// Per report ID: some key down in its last report.
    held: [(u8, bool); REPORTS],
    press: Option<Press>,
    /// Nothing down since then, in a press.
    quiet_since: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    from: (u16, u16),
    to: (u16, u16),
}

#[derive(Clone, Copy, Debug, Default)]
struct Press {
    key: Option<Code>,
    stroke: Option<Finger>,
}

impl Presses {
    pub fn new(layout: Layout) -> Self {
        Presses { layout, finger: None, held: [(0, false); REPORTS], press: None, quiet_since: None }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// A report notified (`report`: its ID, from its Report Reference; `data` without it). A press ended by it
    /// comes out of [`Presses::poll`] once quiet.
    pub fn report(&mut self, report: u8, data: &[u8], now_ms: u64) {
        let mut active = false;
        if let Some(t) = self.layout.touch.filter(|t| t.tip.report == report) {
            active = true;
            let down = t.tip.read(data) == Some(1);
            let at = match (t.x.read(data), t.y.read(data)) {
                (Some(x), Some(y)) => Some((scale(x, t.x_max), scale(y, t.y_max))),
                _ => None,
            };
            match (down, self.finger.as_mut(), at) {
                (true, None, Some(at)) => self.finger = Some(Finger { from: at, to: at }),
                (true, Some(f), Some(at)) => f.to = at,
                (false, Some(_), _) => {
                    let f = self.finger.take();
                    // the first stroke of a press is its own (a double tap is one button)
                    let p = self.press.get_or_insert_default();
                    if p.stroke.is_none() {
                        p.stroke = f;
                    }
                }
                _ => {}
            }
            if self.finger.is_some() {
                self.press.get_or_insert_default();
            }
        }
        if let Some((keys, n)) = self.layout.keys::<4>(report, data) {
            active = true;
            let slot = match self.held.iter().position(|(r, _)| *r == report) {
                Some(s) => Some(s),
                None => self.held.iter().position(|(r, h)| *r == 0 && !*h),
            };
            if let Some(s) = slot {
                self.held[s] = (report, n > 0);
            }
            if n > 0 {
                let p = self.press.get_or_insert_default();
                if p.key.is_none() {
                    p.key = Some(keys[0]);
                }
            }
        }
        if !active {
            return;
        }
        let down = self.finger.is_some() || self.held.iter().any(|(_, h)| *h);
        self.quiet_since = match (down, self.press.is_some()) {
            (false, true) => Some(self.quiet_since.unwrap_or(now_ms)),
            _ => None,
        };
    }

    /// When to call [`Presses::poll`] next, if a press is ending.
    pub fn deadline(&self) -> Option<u64> {
        self.quiet_since.map(|t| t + QUIET_MS)
    }

    /// The press that ended, once quiet for [`QUIET_MS`].
    pub fn poll(&mut self, now_ms: u64) -> Option<Signature> {
        if self.deadline().is_none_or(|d| now_ms < d) {
            return None;
        }
        self.quiet_since = None;
        let p = self.press.take()?;
        match (p.key, p.stroke) {
            (Some(k), _) => Some(Signature::Key(k)),
            (None, Some(f)) => Some(Signature::Stroke { from: f.from, to: f.to }),
            (None, None) => None,
        }
    }
}

fn scale(v: u32, max: u32) -> u16 {
    (v.min(max) as u64 * SCALE as u64 / max.max(1) as u64) as u16
}
