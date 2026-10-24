//! Where the rider is on the route, from the GNSS fixes: the heading (the course while moving, the last one while
//! stopped) and the progress along the route (kept from fix to fix, so that a loop or an out-and-back stays on
//! the same pass).

use route::{Progress, Route};

use crate::fonts::Line;
use crate::{OFF_ROUTE_DM, RouteView};

/// Above this speed the GNSS course is the heading; below, the last one stays.
pub const MOVING_MM_S: u32 = 1_000;

/// Whether a route position is good enough to show distances to climbs. No fix means its last progress is stale; a
/// current fix farther from the route than the off-route tolerance cannot locate the rider along it reliably.
pub fn has_confident_route_position(fix: bool, progress: Option<Progress>) -> bool {
    fix && progress.is_some_and(|p| p.off_dm <= OFF_ROUTE_DM)
}

/// A GNSS fix, what the navigation needs of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fix {
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub speed_mm_s: Option<u32>,
    pub course_cdeg: Option<u16>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nav {
    heading_cdeg: Option<u16>,
    progress: Option<Progress>,
}

impl Nav {
    /// The route as the screens show it, after the fix (`None`: no fix now).
    pub fn update(&mut self, r: &Route<'_>, fix: Option<Fix>) -> RouteView {
        let me = fix.map(|f| r.project(f.lat_e7, f.lon_e7));
        if let Some(f) = fix
            && f.speed_mm_s.is_some_and(|v| v >= MOVING_MM_S)
            && f.course_cdeg.is_some()
        {
            self.heading_cdeg = f.course_cdeg;
        }
        if let Some((x, y)) = me {
            self.progress = Some(r.locate(x, y, self.progress));
        }
        RouteView {
            name: Line::new(format_args!("{}", r.name())),
            length_dm: r.info.length_dm,
            ascent_m: r.info.ascent_m,
            me,
            heading_cdeg: self.heading_cdeg,
            progress: self.progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(off_dm: u32) -> Option<Progress> {
        Some(Progress {
            along_dm: 12_345,
            off_dm,
            segment: 0,
        })
    }

    #[test]
    fn route_distances_need_a_current_fix_within_the_off_route_tolerance() {
        assert!(
            !has_confident_route_position(false, progress(0)),
            "no current fix"
        );
        assert!(
            !has_confident_route_position(true, None),
            "no route progress yet"
        );
        assert!(has_confident_route_position(true, progress(0)));
        assert!(
            has_confident_route_position(true, progress(OFF_ROUTE_DM)),
            "tolerance included"
        );
        assert!(
            !has_confident_route_position(true, progress(OFF_ROUTE_DM + 1)),
            "outside tolerance"
        );
    }
}
