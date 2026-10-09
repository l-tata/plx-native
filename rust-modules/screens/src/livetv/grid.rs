//! **The guide's cursor**: which channel row, which moment, and which slice of time and rows is on
//! screen. Pure — no drawing, no clock read, no store: every method takes the lineup and the wall
//! time it needs, so the movement rules are host-tested exactly as the remote drives them.
//!
//! The cursor is a MOMENT, not a cell index. Rows have airings of different lengths at different
//! offsets, so "the cell under the cursor" is whatever airing covers `at_ms` on the focused row:
//! UP/DOWN keep the moment (the cursor lands on whatever is on at that time on the next channel,
//! the way every TV guide behaves), LEFT/RIGHT move it to the neighbouring airing's start. Until
//! the viewer moves sideways the cursor FOLLOWS now, so a guide left open keeps pointing at what
//! is on.

use plx_data::livetv::guide::{Channel, Lineup};

/// One guide column: the grid's time axis is labelled every half hour.
pub const SLOT_MS: i64 = 30 * 60 * 1000;
/// How much time the grid shows at once.
pub const WINDOW_MS: i64 = 2 * 60 * 60 * 1000;
/// How far ahead the cursor may travel — the store keeps a day of guide.
pub const AHEAD_MS: i64 = plx_data::livetv::source::GUIDE_AHEAD_MS;

/// `t` rounded down to its half hour.
pub fn floor_slot(t: i64) -> i64 {
    t.div_euclid(SLOT_MS) * SLOT_MS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The focused channel row.
    pub row: usize,
    /// The focused moment (epoch ms). Never before now.
    pub at_ms: i64,
    /// The left edge of the visible time window, on a half hour.
    pub window_ms: i64,
    /// The first visible row.
    pub top: usize,
    /// Whether `at_ms` tracks the wall clock (true until the viewer moves sideways).
    pub follow: bool,
}

impl Cursor {
    pub fn new(now: i64) -> Self {
        Self { row: 0, at_ms: now, window_ms: floor_slot(now), top: 0, follow: true }
    }

    /// Time passed. A following cursor moves with it; any cursor is kept at or after now, and the
    /// window never shows a half hour that is entirely over. `true` when anything moved.
    pub fn tick(&mut self, now: i64) -> bool {
        let before = *self;
        if self.follow || self.at_ms < now {
            self.at_ms = now;
        }
        self.window_ms = self.window_ms.max(floor_slot(now));
        self.reveal_time();
        *self != before
    }

    /// Clamp to a lineup that may have shrunk, and keep the focused row visible.
    pub fn fit(&mut self, lineup: &Lineup, visible: usize) {
        self.row = self.row.min(lineup.len().saturating_sub(1));
        self.reveal_row(visible);
    }

    /// RIGHT: the start of the next airing on this row, or one half hour on when the row has no
    /// listing there. `false` at the end of the guide.
    pub fn right(&mut self, lineup: &Lineup, now: i64) -> bool {
        let limit = now + AHEAD_MS;
        let next = match lineup.channels.get(self.row) {
            Some(c) => next_start(c, self.at_ms).unwrap_or(floor_slot(self.at_ms) + SLOT_MS),
            None => return false,
        };
        if next >= limit {
            return false;
        }
        self.at_ms = next;
        self.follow = false;
        self.reveal_time();
        true
    }

    /// LEFT: the start of the airing before the focused one, never before now (the airing on now
    /// is as far back as a live guide goes). `false` when already there.
    pub fn left(&mut self, lineup: &Lineup, now: i64) -> bool {
        let Some(c) = lineup.channels.get(self.row) else { return false };
        let here = c.airing_at(self.at_ms).map(|i| c.airings[i].start_ms).unwrap_or(floor_slot(self.at_ms));
        // The airing containing `now` (or now itself) is the floor.
        let floor = c.airing_at(now).map(|i| c.airings[i].start_ms).unwrap_or(now);
        if here <= floor {
            if self.at_ms != now {
                self.at_ms = now;
                self.follow = true;
                self.reveal_time();
                return true;
            }
            return false;
        }
        let prev = c
            .airings
            .iter()
            .rev()
            .find(|a| a.start_ms < here)
            .map(|a| a.start_ms)
            .unwrap_or(here - SLOT_MS);
        self.at_ms = prev.max(now);
        self.follow = self.at_ms == now;
        self.reveal_time();
        true
    }

    /// UP/DOWN by `delta` rows, keeping the moment. `false` when the move would leave the lineup
    /// (an UP on the first row is the page's to hand to the strip).
    pub fn step_row(&mut self, lineup: &Lineup, delta: i32, visible: usize) -> bool {
        let n = lineup.len();
        let Some(to) = (self.row as i64).checked_add(i64::from(delta)) else { return false };
        if to < 0 || to as usize >= n || n == 0 {
            return false;
        }
        self.row = to as usize;
        self.reveal_row(visible);
        true
    }

    /// CH▲ / CH▼: a page of rows, clamped to the lineup (unlike [`Self::step_row`], a page always
    /// lands somewhere while there is anywhere to land).
    pub fn page(&mut self, lineup: &Lineup, dir: i32, visible: usize) -> bool {
        let n = lineup.len();
        if n == 0 {
            return false;
        }
        let jump = visible.max(1) as i64 * i64::from(dir.signum());
        let to = (self.row as i64 + jump).clamp(0, n as i64 - 1) as usize;
        if to == self.row {
            return false;
        }
        self.row = to;
        self.reveal_row(visible);
        true
    }

    /// Put the cursor on `row` (a typed channel number, a restored channel).
    pub fn go_to_row(&mut self, row: usize, visible: usize) {
        self.row = row;
        self.reveal_row(visible);
    }

    fn reveal_row(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + visible {
            self.top = self.row + 1 - visible;
        }
    }

    /// Keep the focused moment inside the window, with a half hour of context to its left once
    /// the window has to move right.
    fn reveal_time(&mut self) {
        if self.at_ms < self.window_ms {
            self.window_ms = floor_slot(self.at_ms);
        } else if self.at_ms >= self.window_ms + WINDOW_MS - SLOT_MS {
            self.window_ms = floor_slot(self.at_ms) - SLOT_MS;
        }
    }
}

/// The start of the first airing after the one covering `t` (or after `t` itself in a gap).
fn next_start(c: &Channel, t: i64) -> Option<i64> {
    match c.airing_at(t) {
        Some(i) => c.airings.get(i + 1).map(|a| a.start_ms).or(Some(c.airings[i].stop_ms)),
        None => c.airings.iter().find(|a| a.start_ms > t).map(|a| a.start_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::livetv::guide::Airing;

    const MIN: i64 = 60 * 1000;
    /// 2026-10-09T20:10Z — ten minutes into the 20:00 half hour.
    const NOW: i64 = 1_791_576_600_000;

    fn airing(start_min: i64, len_min: i64) -> Airing {
        let base = floor_slot(NOW);
        Airing { start_ms: base + start_min * MIN, stop_ms: base + (start_min + len_min) * MIN, ..Default::default() }
    }

    fn lineup() -> Lineup {
        let ch = |n: &str, airings: Vec<Airing>| Channel { number: n.into(), airings, ..Default::default() };
        Lineup {
            channels: vec![
                ch("1", vec![airing(0, 30), airing(30, 60), airing(90, 30)]),
                ch("2", vec![airing(-30, 120)]),
                ch("3", vec![]),
                ch("4", vec![airing(0, 30)]),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_new_cursor_follows_now_until_moved_sideways() {
        let mut c = Cursor::new(NOW);
        assert_eq!((c.row, c.at_ms, c.window_ms), (0, NOW, floor_slot(NOW)));
        assert!(c.tick(NOW + 5 * MIN));
        assert_eq!(c.at_ms, NOW + 5 * MIN);
        let l = lineup();
        assert!(c.right(&l, NOW + 5 * MIN));
        assert!(!c.follow);
        let at = c.at_ms;
        c.tick(NOW + 6 * MIN);
        assert_eq!(c.at_ms, at, "a moved cursor stays on the airing it was moved to");
    }

    #[test]
    fn right_walks_airings_and_left_walks_back_to_now() {
        let l = lineup();
        let mut c = Cursor::new(NOW);
        assert!(c.right(&l, NOW));
        assert_eq!(c.at_ms, airing(30, 60).start_ms);
        assert!(c.right(&l, NOW));
        assert_eq!(c.at_ms, airing(90, 30).start_ms);
        assert!(c.left(&l, NOW));
        assert_eq!(c.at_ms, airing(30, 60).start_ms);
        assert!(c.left(&l, NOW));
        assert_eq!(c.at_ms, NOW, "back on the airing that is on: the cursor is now again");
        assert!(c.follow);
        assert!(!c.left(&l, NOW), "nothing before now");
    }

    #[test]
    fn up_and_down_keep_the_moment_and_stop_at_the_ends() {
        let l = lineup();
        let mut c = Cursor::new(NOW);
        c.right(&l, NOW);
        let at = c.at_ms;
        assert!(c.step_row(&l, 1, 3));
        assert_eq!((c.row, c.at_ms), (1, at));
        assert!(!c.step_row(&l, -2, 3), "an UP past the first row is the strip's");
        assert!(c.step_row(&l, 2, 3));
        assert_eq!(c.row, 3);
        assert_eq!(c.top, 1, "the row was revealed");
        assert!(!c.step_row(&l, 1, 3));
    }

    #[test]
    fn an_empty_row_steps_by_half_hours() {
        let l = lineup();
        let mut c = Cursor::new(NOW);
        c.go_to_row(2, 4);
        assert!(c.right(&l, NOW));
        assert_eq!(c.at_ms, floor_slot(NOW) + SLOT_MS);
        assert!(c.left(&l, NOW));
        assert_eq!(c.at_ms, NOW);
    }

    #[test]
    fn the_window_scrolls_to_keep_the_cursor_and_never_shows_the_past() {
        let l = lineup();
        let mut c = Cursor::new(NOW);
        for _ in 0..3 {
            c.right(&l, NOW);
        }
        // airing(90,30) ends at +120; the next RIGHT on row 1 lands at its stop.
        assert!(c.at_ms < c.window_ms + WINDOW_MS);
        assert!(c.window_ms >= floor_slot(NOW));
        c.tick(NOW + 3 * SLOT_MS);
        assert!(c.window_ms >= floor_slot(NOW + 3 * SLOT_MS));
        assert!(c.at_ms >= NOW + 3 * SLOT_MS);
    }

    #[test]
    fn paging_clamps_and_fit_survives_a_shrunk_lineup() {
        let l = lineup();
        let mut c = Cursor::new(NOW);
        assert!(c.page(&l, 1, 3));
        assert_eq!(c.row, 3);
        assert!(!c.page(&l, 1, 3));
        assert!(c.page(&l, -1, 3));
        assert_eq!(c.row, 0);
        c.row = 3;
        c.fit(&Lineup { channels: l.channels[..1].to_vec(), ..Default::default() }, 3);
        assert_eq!((c.row, c.top), (0, 0));
    }
}
