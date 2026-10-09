//! The deck's refetch schedule, rule by rule (`deck_refresh.rs`'s module doc names them).

use super::*;

/// Step `d` from `t` with Home visible and nothing in the way; how many refetches were issued.
fn run(d: &mut DeckRefresh, from: u32, to: u32, every: u32, home: bool) -> usize {
    let mut issued = 0;
    let mut t = from;
    while t <= to {
        issued += usize::from(d.step(t, home, true, false));
        t += every;
    }
    issued
}

#[test]
fn a_stopped_playback_refetches_once_after_the_settle_beat() {
    let mut d = DeckRefresh::new(0);
    d.playback_stopped(10_000 + PLAYBACK_SETTLE_MS);
    assert!(!d.step(10_000, false, true, false), "not before the timeline PUT has landed");
    assert!(d.step(10_000 + PLAYBACK_SETTLE_MS, false, true, false), "the deck follows the stop");
    assert!(!d.step(10_000 + PLAYBACK_SETTLE_MS + 16, false, true, false), "and only once");
}

#[test]
fn nothing_is_issued_under_the_player_and_the_debt_survives_it() {
    let mut d = DeckRefresh::new(0);
    d.playback_stopped(1_000);
    assert!(!d.step(2_000, false, false, false), "the route is still the player");
    assert!(d.owed());
    assert!(d.step(2_016, false, true, false), "paid the frame the player has gone");
}

#[test]
fn a_reason_during_a_fetch_waits_for_it_and_is_paid_once() {
    let mut d = DeckRefresh::new(0);
    d.foreground(500);
    for t in (500..5_000).step_by(16) {
        assert!(!d.step(t, true, true, true), "never a second request while one is in flight");
    }
    // A second reason arrives while the first still waits: one debt, one request.
    d.playback_stopped(5_000);
    assert!(d.step(5_016, true, true, false), "the landing frees the one owed request");
    assert!(!d.step(5_032, true, true, false));
    assert!(!d.owed(), "both reasons were paid by the one request");
}

#[test]
fn home_shown_again_refetches_only_a_deck_older_than_a_minute() {
    let mut d = DeckRefresh::new(0);
    assert!(!d.step(1_000, true, true, false), "boot fetched it a second ago");
    assert!(!d.step(2_000, false, true, false), "a detail page opens over Home");
    assert!(!d.step(30_000, true, true, false), "back within the minute: still fresh");
    assert!(!d.step(31_000, false, true, false));
    assert!(d.step(SHOWN_STALE_MS + 1_000, true, true, false), "back after a minute: refetched");
}

#[test]
fn a_visible_home_refetches_on_the_two_minute_cadence() {
    let mut d = DeckRefresh::new(0);
    // Ten minutes of a Home left on screen, a frame every 16 ms.
    let issued = run(&mut d, 0, 10 * 60 * 1000, 16, true);
    assert_eq!(issued as u32, 10 * 60 * 1000 / VISIBLE_CADENCE_MS, "one refetch per cadence");
}

#[test]
fn a_hidden_home_is_not_polled() {
    let mut d = DeckRefresh::new(0);
    assert_eq!(run(&mut d, 0, 30 * 60 * 1000, 16, false), 0, "no cadence off Home");
}

#[test]
fn the_foreground_refetches_at_once() {
    let mut d = DeckRefresh::new(0);
    d.foreground(5_000);
    assert!(d.step(5_000, false, true, false));
}

#[test]
fn an_earlier_debt_is_kept_when_a_later_one_arrives() {
    let mut d = DeckRefresh::new(0);
    d.foreground(1_000);
    d.playback_stopped(9_000);
    assert!(d.step(1_000, false, true, false), "the earlier reason sets the time");
}

#[test]
fn the_schedule_reads_the_frame_clock_across_its_wrap() {
    let start = u32::MAX - 30_000;
    let mut d = DeckRefresh::new(start);
    d.playback_stopped(start.wrapping_add(PLAYBACK_SETTLE_MS));
    assert!(!d.step(start, false, true, false));
    assert!(d.step(start.wrapping_add(PLAYBACK_SETTLE_MS), false, true, false));
    assert!(d.step(start.wrapping_add(VISIBLE_CADENCE_MS + PLAYBACK_SETTLE_MS), true, true, false),
        "the cadence counts across the wrap");
}

/// **The deck after a stop is read twice**: at the settle beat, and again once the scrobble
/// worker's late `stopped` timeline has surely landed — otherwise a deck read before it would stand
/// until the next two-minute tick.
#[test]
fn a_stop_is_followed_by_a_second_refetch() {
    let mut d = DeckRefresh::new(0);
    d.playback_stopped(10_000 + PLAYBACK_SETTLE_MS);
    assert!(d.step(10_000 + PLAYBACK_SETTLE_MS, true, true, false), "the first refetch at the settle beat");
    assert!(!d.step(10_000 + PLAYBACK_SETTLE_MS + 100, true, true, false));
    assert!(!d.step(10_000 + PLAYBACK_FOLLOWUP_MS - 1, true, true, false), "not before the follow-up");
    assert!(d.step(10_000 + PLAYBACK_FOLLOWUP_MS + 100, true, true, false), "the follow-up refetch");
    assert!(!d.step(10_000 + PLAYBACK_FOLLOWUP_MS + 200, true, true, false), "and only one");
}
