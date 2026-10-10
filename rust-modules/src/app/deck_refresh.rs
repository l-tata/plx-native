//! **When Home refetches Continue Watching** — the one schedule for every reason the deck can be
//! out of date, so the reasons cannot fight over the server.
//!
//! The deck (`/hubs/continueWatching`, refetched with the rest of Home's hubs by
//! `HubsCmd::RefetchHubs`) moves whenever a resume point moves, and a resume point moves in four
//! places this app can see:
//!
//! - **a playback stopped** (Stop, BACK, end of stream, a live channel left — every exit goes
//!   through `playback::exit_player`, which arms `App::refresh_hubs_at`). The refetch is owed a
//!   beat after the stop ([`PLAYBACK_SETTLE_MS`] in the arming code) so the final timeline PUT has
//!   landed server-side first, and once more [`PLAYBACK_FOLLOWUP_MS`] later, because the stop's
//!   final `stopped` timeline is sent by a worker and can land after the first read;
//! - **Home is shown again after another page** and its deck is older than [`SHOWN_STALE_MS`] —
//!   something may have been watched on another device, or on this one through a page that does
//!   not stop through the player;
//! - **the app returns to the foreground** — the set slept, or another app was in front, for an
//!   unknown length of time;
//! - **Home stays on screen**: every [`VISIBLE_CADENCE_MS`] while it is the visible page.
//!
//! Every reason only OWES a refetch; [`DeckRefresh::step`] decides when one is issued, and it never
//! issues one while a hub fetch is still in flight. A reason that arrives during a fetch is not
//! dropped and not doubled: it stays owed and is paid once, as soon as that fetch lands. That is
//! the "never more than one in flight" rule, and it is also what keeps a burst of reasons (a stop
//! that returns to Home just as the app is foregrounded) to one request — besides the stop's own
//! follow-up read.
//!
//! Nothing here blanks a shelf: a refetch keeps the published catalog until the new one lands
//! (`pms::step_landings_with_scope` commits only a landing), and Home keys its focus by item
//! identity, so a deck that still holds the focused card keeps the focus on it.
//!
//! Pure: no clock, no store, no route — the loop hands every input in (`run.rs`), so the rules are
//! graded on the host.

/// How long a deck is trusted when Home is shown again after another page.
pub(crate) const SHOWN_STALE_MS: u32 = 60 * 1000;

/// How often the deck is refetched while Home stays the visible page.
pub(crate) const VISIBLE_CADENCE_MS: u32 = 2 * 60 * 1000;

/// The beat between a playback stopping and its refetch, so the final timeline PUT lands first.
pub(crate) const PLAYBACK_SETTLE_MS: u32 = 800;

/// **A second refetch this long after a stop.** The final `stopped` timeline is sent by the
/// scrobble worker, which first joins the progress reporter — a request that may itself be in
/// flight — so it can land seconds after the settle beat. A deck read in that window is the old
/// one, and Home would show it until the next two-minute tick.
pub(crate) const PLAYBACK_FOLLOWUP_MS: u32 = 6_000;

/// The deck's refetch schedule. Frame-clock milliseconds throughout, compared wrapping.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DeckRefresh {
    /// When this schedule last issued a refetch (or the boot fetch was issued).
    asked_at: u32,
    /// A refetch is owed from this time on, and has not been issued yet.
    owed_at: Option<u32>,
    /// Was Home the visible page on the previous step? Its rising edge is "Home shown again".
    home_visible: bool,
    /// A stop's follow-up refetch, owed once the first one has been paid.
    followup_at: Option<u32>,
}

/// `a` is at or after `b` on the wrapping frame clock.
fn reached(now: u32, at: u32) -> bool {
    now.wrapping_sub(at) < 0x8000_0000
}

impl DeckRefresh {
    /// A schedule whose deck was fetched at `now` (the boot fetch).
    pub(crate) fn new(now: u32) -> Self {
        Self { asked_at: now, owed_at: None, home_visible: false, followup_at: None }
    }

    /// Owe a refetch from `at` on, keeping an earlier debt if one is already owed.
    fn owe(&mut self, at: u32) {
        self.owed_at = Some(match self.owed_at {
            Some(owed) if reached(at, owed) => owed,
            _ => at,
        });
    }

    /// A playback stopped and its refetch is owed from `at` (the stop plus the settle beat).
    pub(crate) fn playback_stopped(&mut self, at: u32) {
        self.owe(at);
        self.followup_at = Some(at.wrapping_add(PLAYBACK_FOLLOWUP_MS - PLAYBACK_SETTLE_MS.min(PLAYBACK_FOLLOWUP_MS)));
    }

    /// The app came back to the foreground at `now`.
    pub(crate) fn foreground(&mut self, now: u32) {
        self.owe(now);
    }

    /// One frame at `now`. `home_visible`: Home is the page on screen. `may_fetch`: nothing stands
    /// in the way of a refetch (the player is not the route — a refetch under a playing film is
    /// wasted work the exit will owe again). `in_flight`: a hub fetch has not landed yet.
    ///
    /// Returns `true` exactly when the caller must issue ONE refetch now; the schedule has then
    /// recorded it as asked.
    pub(crate) fn step(&mut self, now: u32, home_visible: bool, may_fetch: bool, in_flight: bool) -> bool {
        let age = now.wrapping_sub(self.asked_at);
        if home_visible && !self.home_visible && age >= SHOWN_STALE_MS {
            self.owe(now);
        }
        self.home_visible = home_visible;
        if home_visible && age >= VISIBLE_CADENCE_MS {
            self.owe(now);
        }
        // A stop's follow-up comes due once the first refetch has been paid.
        if self.owed_at.is_none() {
            if let Some(at) = self.followup_at.filter(|&at| reached(now, at)) {
                self.followup_at = None;
                self.owe(at);
            }
        }
        let due = self.owed_at.is_some_and(|at| reached(now, at));
        if !due || !may_fetch || in_flight {
            return false;
        }
        self.owed_at = None;
        self.asked_at = now;
        true
    }

    /// Is a refetch owed (due or not)? For the log and the tests.
    #[cfg(test)]
    pub(crate) fn owed(&self) -> bool {
        self.owed_at.is_some()
    }
}

#[cfg(test)]
#[path = "deck_refresh_tests.rs"]
mod tests;
