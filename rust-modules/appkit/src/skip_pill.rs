//! The **Skip Intro / Skip Credits** control. While the playhead sits inside one of the playing
//! leaf's server markers (`metadata::playing_markers`, fetched with `?includeMarkers=1`) this
//! button TAKES THE PLACE of the transport's Subtitles/Audio discs — same row, same right edge,
//! same height — and the discs return the moment the segment ends.
//!
//! Replacing them rather than joining them is the design call: one wide unmissable target in the
//! spot the eye already goes for transport controls. The cost is that CC/Audio are unreachable for
//! the length of the segment, and that the button only exists while the HUD is up — which is why
//! `app.rs` RAISES the HUD once when a segment begins and parks focus here, so a bare OK still
//! skips in one press instead of three.
#![allow(dead_code)]
use plx_data::metadata::{self, MarkerKind};
use plx_ui::theme;
use plx_ui::widgets::{Button, ControlGround};
use plx_ui::{Env, Painter, Rect, View};
use std::ffi::CString;

/// What pressing the button does. The distinction is the `final` flag on a credits marker: an
/// ordinary segment is a seek and playback continues past it, but a `final` one runs to the end of
/// the item, so "skip" means **the episode is over** — seeking to its end would race the decoder
/// against its own last frames for no benefit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkipAction {
    /// jump to this position (ns) and keep playing
    Seek(i64),
    /// the item is finished — hand off to the end-of-playback path (next episode, or leave)
    Finish,
    /// **Undo an automatic skip**: go back to this position (ns, the segment's start) and keep
    /// playing. Only ever offered by [`undo_prompt`], for [`UNDO_MS`] after the player skipped a
    /// segment by itself (Settings > Playback > Skip intro & credits). The segment stays retired
    /// (`metadata::mark_skipped`), so landing back inside it is watching it, not a new offer.
    Rewind(i64),
}

/// How long the "Skipped intro · Back" pill stands after an automatic skip, and how long LEFT
/// means "take me back" rather than "scrub". Long enough to read the pill and reach for the
/// remote; short enough that LEFT is the scrub key again by the time anyone means to scrub.
pub const UNDO_MS: u32 = 5_000;

/// An automatic skip that can still be taken back: the segment it jumped over, and the frame time
/// (ms, `clock::now`) its window closes. Owned by the player screen's HUD state
/// (`screens::player::input::HudState::undo`); the loop installs it when it performs the skip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AutoSkipUndo {
    pub marker: metadata::Marker,
    pub until: u32,
}

impl AutoSkipUndo {
    /// The undo opened at `now` for `marker`.
    pub fn at(marker: metadata::Marker, now: u32) -> Self {
        AutoSkipUndo { marker, until: now.wrapping_add(UNDO_MS).max(1) }
    }
    /// Is the window still open at `now`? Wrapping-safe, like every other frame-time comparison.
    pub fn live(&self, now: u32) -> bool {
        self.until.wrapping_sub(now) as i32 > 0
    }
    /// Where "Back" lands: the segment's own start, in ns.
    pub fn back_ns(&self) -> i64 {
        self.marker.start_ms * 1_000_000
    }
}

/// The offer the button is currently making. Carries the marker's TYPED kind, not its label:
/// asking "is the playhead in a credits segment?" by string-comparing user-facing copy made a pure
/// copy-edit — the sort of thing a design pass does — silently disable Up Next and auto-advance,
/// with no compile error and nothing to fail.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    /// the segment itself — carried so activating the button can retire it (`metadata::mark_skipped`)
    pub marker: metadata::Marker,
    pub kind: MarkerKind,
    pub action: SkipAction,
}

impl Prompt {
    /// The button's copy — presentation, derived from the kind at the point of drawing.
    pub fn label(&self) -> &'static str {
        match (self.action, self.kind) {
            (SkipAction::Rewind(_), MarkerKind::Intro) => plx_platform::i18n::msg::widgets_skip_intro_skipped(),
            (SkipAction::Rewind(_), MarkerKind::Credits) => plx_platform::i18n::msg::widgets_skip_credits_skipped(),
            (_, MarkerKind::Intro) => plx_platform::i18n::msg::widgets_skip_intro(),
            (_, MarkerKind::Credits) => plx_platform::i18n::msg::widgets_skip_credits(),
        }
    }
}

/// PURE: the offer a given segment makes. Takes the marker rather than reading the playhead, so
/// the precedence it feeds ([`crate::player_hud::slot_for`]) is host-testable and the whole frame
/// decides from ONE playhead sample — `playpos_ns` is written by LG's media thread, and re-reading
/// it per call site let the input path and the draw path disagree within a single frame.
///
/// Markers belong to the PLAYING leaf (`metadata::playing()`), never to `metadata::current()`:
/// during a show-page episode play `current()` is the SHOW, and offering episode 1's intro timing
/// during episode 5 is the same identity bug the track store exists to prevent.
pub fn prompt_for(m: metadata::Marker) -> Prompt {
    Prompt {
        marker: m,
        kind: m.kind,
        action: match m.kind {
            // a `final` credits segment runs to the end of the item, so "skip" means the episode is
            // over — seeking to its end would race the decoder against its own last frames
            MarkerKind::Credits if m.final_seg => SkipAction::Finish,
            _ => SkipAction::Seek(m.end_ms * 1_000_000),
        },
    }
}

/// PURE: the "Skipped intro · Back" offer that stands in the control row after an automatic skip of
/// `m` — the same pill, wearing the undo label and the [`SkipAction::Rewind`] action. Its
/// [`crate::player_hud::ControlSlot::offer`] is the original segment's, so it never counts as a
/// fresh offer and never re-raises the HUD on its own.
pub fn undo_prompt(m: metadata::Marker) -> Prompt {
    Prompt { marker: m, kind: m.kind, action: SkipAction::Rewind(m.start_ms * 1_000_000) }
}

/// PURE: does the Skip-intro-and-credits setting skip this offer by itself?
///
/// Only an ordinary [`SkipAction::Seek`] or [`SkipAction::Finish`] offer qualifies, of a kind the
/// setting names. The Up Next interaction is decided BEFORE this is asked:
/// [`crate::player_hud::slot_for`] never makes a Skip offer out of a final credits segment with a
/// queued successor (Countdown turns it into the Up Next tile, the other modes into the discs), so
/// a `Finish` reaching here is always the LAST item — and finishing it leaves the player instead
/// of silently starting another episode.
pub fn auto_skips(pr: Prompt, mode: plx_plex::plex::session::AutoSkip) -> bool {
    let kind_on = match pr.kind {
        MarkerKind::Intro => mode.skips_intro(),
        MarkerKind::Credits => mode.skips_credits(),
    };
    kind_on && !matches!(pr.action, SkipAction::Rewind(_))
}

/// The button's rect — the SHARED control-row slot, so it and Up Next cannot drift apart.
///
/// `row` is the player instance's own [`crate::player_hud::TransportRow`] (restructure phase
/// 9): it carries both the label-width memo this measurement is cached in and the control row's
/// focus springs. It was a module `static mut` on the other side of `ctrl_slot` until then.
pub fn rect(row: &mut crate::player_hud::TransportRow, pr: Prompt, measure: &dyn plx_machine::machine::Measure) -> Rect {
    crate::player_hud::ctrl_slot(row, pr.label(), measure)
}

/// Draw the button in the control row. Called by `player_hud` INSTEAD of the two discs.
pub fn draw(row: &mut crate::player_hud::TransportRow, p: Painter, pr: Prompt, focused: bool, measure: &dyn plx_machine::machine::Measure) {
    let Ok(label) = CString::new(pr.label()) else {
        return;
    };
    // No leading icon: the label alone carries it, and a chevron on a control that does not
    // navigate anywhere was reading as "more" rather than "skip".
    // Its slot's only item, so index 0 — the pop is the control ROW's (`TransportRow::scale`),
    // shared with the transport discs this pill stands in for.
    let slot = rect(row, pr, measure);
    let pop = row.scale(0);
    Button::new(label.as_ptr(), theme::size::BODY, slot)
        .scale(pop)
        .focused(focused)
        // It stands in the transport discs' own slot, over the video plane and on the HUD's ramp,
        // so it wears their ground as well as their pop — see `ControlGround`.
        .ground(ControlGround::Unkeyed)
        .draw(&Env::inert(), p);
}
