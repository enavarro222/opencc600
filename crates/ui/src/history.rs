//! Altitude along the ride, every 100 m: the gradient, the last 5 km and the whole ride's profile; and every second
//! of the timer over the last minute: the climbing speed.

use crate::{PROFILE, Profile};

/// Distance between two points.
pub const STEP_CM: u32 = 10_000;
/// The climbing speed is the altitude gained over this much of the timer (seconds)...
pub const VAM_S: usize = 60;
/// ...once it has this much: the barometer is within a metre or two, ±120 m/h over 60 s, ±240 over 30.
pub const VAM_MIN_S: u32 = 30;

/// Fed with each record of the ride.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct History {
    /// The last [`PROFILE`] points, a ring: `recent[(start + i) % PROFILE]` is the i-th oldest.
    recent: [i16; PROFILE],
    start: usize,
    len: usize,
    /// One point every `every` steps, halved (every other point kept) when full.
    whole: [i16; PROFILE],
    whole_len: usize,
    every: u32,
    steps: u32,
    /// Distance of the next point.
    next_cm: Option<u32>,
    /// Altitude at each second of the timer, a ring ending at `second`, `seconds` of them.
    by_second: [i16; VAM_S + 1],
    second: u32,
    seconds: u32,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub const fn new() -> Self {
        Self {
            recent: [0; PROFILE],
            start: 0,
            len: 0,
            whole: [0; PROFILE],
            whole_len: 0,
            every: 1,
            steps: 0,
            next_cm: None,
            by_second: [0; VAM_S + 1],
            second: 0,
            seconds: 0,
        }
    }

    /// The altitude at `timer_s` of the timer (once a record): the seconds skipped (a point every 5 s) get it too.
    pub fn push_time(&mut self, timer_s: u32, altitude_dm: i32) {
        let alt = altitude_dm.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let n = self.by_second.len() as u32;
        if self.seconds == 0 {
            self.second = timer_s;
            self.seconds = 1;
        } else if timer_s > self.second {
            let skipped = (timer_s - self.second).min(n);
            for k in 1..=skipped {
                self.by_second[((self.second + k) % n) as usize] = alt;
            }
            self.second = timer_s;
            self.seconds = (self.seconds + skipped).min(n);
        }
        self.by_second[(self.second % n) as usize] = alt;
    }

    /// Climbing speed, metres an hour: the altitude gained over the last minute of the timer (half a minute at
    /// least).
    pub fn vam_m_h(&self) -> Option<i32> {
        let span = self.seconds.checked_sub(1)?;
        if span < VAM_MIN_S {
            return None;
        }
        let n = self.by_second.len() as u32;
        let now = self.by_second[(self.second % n) as usize] as i32;
        let then = self.by_second[((self.second + n - span) % n) as usize] as i32;
        // decimetres in `span` seconds: × 3600 / 10
        Some((now - then) * 360 / span as i32)
    }

    pub fn push(&mut self, distance_cm: u32, altitude_dm: i32) {
        let alt = altitude_dm.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let next = *self.next_cm.get_or_insert(distance_cm);
        let mut next = next;
        while distance_cm >= next {
            self.add(alt);
            next += STEP_CM;
        }
        self.next_cm = Some(next);
    }

    fn add(&mut self, alt: i16) {
        if self.len < PROFILE {
            self.recent[(self.start + self.len) % PROFILE] = alt;
            self.len += 1;
        } else {
            self.recent[self.start] = alt;
            self.start = (self.start + 1) % PROFILE;
        }
        if self.steps.is_multiple_of(self.every) {
            if self.whole_len == PROFILE {
                for i in 0..PROFILE / 2 {
                    self.whole[i] = self.whole[2 * i];
                }
                self.whole_len = PROFILE / 2;
                self.every *= 2;
            }
            if self.steps.is_multiple_of(self.every) {
                self.whole[self.whole_len] = alt;
                self.whole_len += 1;
            }
        }
        self.steps += 1;
    }

    fn recent_at(&self, i: usize) -> i16 {
        self.recent[(self.start + i) % PROFILE]
    }

    /// The last 5 km, oldest first.
    pub fn recent(&self) -> Profile {
        let mut p = Profile::default();
        for i in 0..self.len {
            p.points[i] = self.recent_at(i);
        }
        p.len = self.len as u8;
        p
    }

    /// The whole ride, at most [`PROFILE`] points.
    pub fn whole(&self) -> Profile {
        let mut p = Profile {
            points: self.whole,
            len: self.whole_len as u8,
        };
        p.points[self.whole_len..].fill(0);
        p
    }

    /// Gradient from the point 100 to 200 m back to here, percent.
    pub fn grade_pct(&self, distance_cm: u32, altitude_dm: i32) -> Option<i8> {
        let next = self.next_cm?;
        if self.len < 2 {
            return None;
        }
        // the second newest point is between 100 and 200 m back
        let back_cm = distance_cm + 2 * STEP_CM - next;
        let from = self.recent_at(self.len - 2) as i32;
        let pct = (altitude_dm - from) as i64 * 1000 / back_cm.max(1) as i64;
        Some(pct.clamp(-99, 99) as i8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient() {
        let mut h = History::new();
        assert_eq!(h.grade_pct(0, 1000), None);
        // 5 % for a kilometre, every 10 m
        for i in 0..=100u32 {
            let d = i * 1000;
            let alt = 1000 + (i * 5) as i32;
            h.push(d, alt);
            if i >= 20 {
                assert_eq!(h.grade_pct(d, alt), Some(5), "at {i}");
            }
        }
        // then flat
        for i in 101..=130u32 {
            h.push(i * 1000, 1500);
        }
        assert_eq!(h.grade_pct(130_000, 1500), Some(0));
    }

    #[test]
    fn climbing_speed() {
        let mut h = History::new();
        // 0.25 m/s: 900 m/h, after half a minute
        for t in 0..30 {
            h.push_time(t, 5000 + t as i32 * 25 / 10);
            assert_eq!(h.vam_m_h(), None, "at {t}");
        }
        for t in 30..200 {
            h.push_time(t, 5000 + t as i32 * 25 / 10);
            let v = h.vam_m_h().unwrap();
            assert!((v - 900).abs() <= 15, "{v} at {t}");
        }
        // a point every 5 s: the same
        for t in (200..400).step_by(5) {
            h.push_time(t, 5000 + t as i32 * 25 / 10);
        }
        assert!((h.vam_m_h().unwrap() - 900).abs() <= 15);
        // stopped on the flat for a minute (the timer runs): 0
        for t in 400..470 {
            h.push_time(t, 6000);
        }
        assert_eq!(h.vam_m_h(), Some(0));
        // the same second twice keeps the last altitude
        h.push_time(470, 6010);
        h.push_time(470, 6020);
        assert_eq!(h.vam_m_h(), Some(20 * 360 / 60));
    }

    #[test]
    fn the_last_5_km_and_the_whole_ride() {
        let mut h = History::new();
        // 23 km, one point every 100 m: 231 points
        for i in 0..=2300u32 {
            h.push(i * 1000, (i / 10) as i32);
        }
        let recent = h.recent();
        assert_eq!(recent.len as usize, PROFILE);
        // the newest point is the 23rd km's, 5 km of points back from it
        assert_eq!(*recent.points().last().unwrap(), 230);
        assert_eq!(recent.points()[0], 230 - PROFILE as i16 + 1);
        let whole = h.whole();
        assert!(
            whole.len as usize > PROFILE / 2 && whole.len as usize <= PROFILE,
            "{}",
            whole.len
        );
        // it starts at the start and goes in equal steps
        assert_eq!(whole.points()[0], 0);
        let step = whole.points()[1] - whole.points()[0];
        assert!(whole.points().windows(2).all(|w| w[1] - w[0] == step));
    }
}
