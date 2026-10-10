//! **The live banner**: the player's transport for a Live TV channel (`plx_media::live`).
//!
//! A channel has no file to scrub, no chapters and no tracks the server chose, so the transport's
//! scrubber, clocks and control row describe nothing; the banner replaces them on the same dark
//! ground (`player_hud::draw_scrim`) and at the same left edge: the `LIVE` badge and the channel,
//! the airing on now in the transport's display face, its wall-clock span and how far through it
//! is, and what follows. While a channel is being tuned it says so; when the tune failed it says
//! that and that OK retries. A virtual channel paused behind live wears a BEHIND badge in place
//! of LIVE and says that OK jumps back to live.
//!
//! Drawn by `screens::player` whenever the session is live and the banner is up (the HUD's own
//! auto-hide timer), plus the typed-digits read-out at the top right while a number is being
//! keyed in, and the channel list ([`draw_surf`]) while the viewer surfs with UP/DOWN — the
//! channel that is playing keeps playing under it until OK tunes the highlighted one.

use crate::player_hud::{draw_scrim, sb_w, SB_H, SB_X, SB_Y};
use plx_data::livetv::guide::{Airing, Lineup};
use plx_media::live::LiveSession;
use plx_ui::channel_tile::ChannelTile;
use plx_ui::label::{HAlign, Label};
use plx_ui::{theme, Painter, Rect};

/// The badge's inner padding and corner, and the gap to the channel text.
const BADGE_PAD: f32 = 12.0;
const BADGE_H: f32 = 36.0;

fn line(p: Painter, measure: &dyn plx_machine::machine::Measure, text: &str, sz: i32, col: [f32; 4], bold: bool, frame: Rect, h: HAlign) -> f32 {
    if text.is_empty() || frame.w <= 0.0 {
        return 0.0;
    }
    let fitted = measure.fit_line(text, frame.w, sz, bold);
    let label = Label::new(fitted.as_ptr(), sz, col).h(h);
    if bold { label.bold().draw(p, frame) } else { label.draw(p, frame) }
}

/// What the banner says about the stream itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The probe is running.
    Tuning,
    /// The probe or the stream failed.
    Unavailable,
    Playing,
}

/// The banner for `live` at wall time `now`.
pub fn draw(live: &LiveSession, phase: Phase, now: i64, measure: &dyn plx_machine::machine::Measure) {
    let p = Painter::root();
    draw_scrim(p);
    let Some(ch) = live.channel() else { return };
    let (on_now, next) = ch.now_next(now);
    let w = sb_w();
    // Row 1: LIVE badge, channel number and name.
    let row1 = SB_Y - 190.0;
    // A virtual channel paused behind live says so, and that OK jumps back.
    let behind = phase == Phase::Playing && live.is_behind(now, plx_media::player::playpos_ns());
    let badge_text = if behind { plx_platform::i18n::msg::livetv_behind() } else { plx_platform::i18n::msg::livetv_live() };
    let badge_w = measure.width_str(badge_text, theme::size::CAPTION, true) + 2.0 * BADGE_PAD;
    let badge = Rect::new(SB_X, row1, badge_w, BADGE_H);
    p.rect(badge, theme::space::XS, theme::LIVE_BADGE, theme::LIVE_BADGE, 0.0);
    line(p, measure, badge_text, theme::size::CAPTION, theme::LIVE_BADGE_INK, true, badge, HAlign::Center);
    let channel = if behind {
        format!("{}  {}  \u{b7}  {}", ch.number, ch.name, plx_platform::i18n::msg::livetv_jump_to_live())
    } else {
        format!("{}  {}", ch.number, ch.name)
    };
    line(p, measure, &channel, theme::size::BODY, theme::TEXT_SECONDARY, true,
        Rect::new(badge.x + badge.w + theme::space::SM, row1, w - badge.w - theme::space::SM, BADGE_H), HAlign::Left);
    // Row 2: the airing's title in the transport's display face (HEADLINE: the HUD's own display
    // size is a documented carve-out this banner does not need).
    let title_y = row1 + BADGE_H + theme::space::SM;
    let title_h = measure.line_h(theme::size::TITLE);
    let title: &str = match phase {
        Phase::Tuning => plx_platform::i18n::msg::livetv_tuning(),
        Phase::Unavailable => plx_platform::i18n::msg::livetv_unavailable(),
        Phase::Playing => on_now.map(|a| a.title.as_str()).filter(|t| !t.is_empty()).unwrap_or(ch.name.as_str()),
    };
    line(p, measure, title, theme::size::TITLE, theme::TEXT_PRIMARY, true, Rect::new(SB_X, title_y, w, title_h), HAlign::Left);
    // Row 3: the progress bar of the airing on now, with its span either side.
    if let Some(a) = on_now {
        draw_progress(p, a, now, measure);
    }
    // Row 4: the airing's facts, and what is next.
    let facts_y = SB_Y + SB_H + theme::space::MD;
    let body_h = measure.line_h(theme::size::BODY);
    let mut facts: Vec<&str> = Vec::new();
    if let Some(a) = on_now {
        for part in [&a.episode, &a.sub_title] {
            if !part.is_empty() {
                facts.push(part.as_str());
            }
        }
    }
    let half = w * 0.5;
    line(p, measure, &facts.join(" \u{b7} "), theme::size::BODY, theme::TEXT_SECONDARY, false,
        Rect::new(SB_X, facts_y, half - theme::space::MD, body_h), HAlign::Left);
    if let Some(n) = next {
        let text = format!("{} \u{b7} {}", plx_ui::fmt::wall_time(n.start_ms), plx_platform::i18n::msg::livetv_next(&n.title));
        line(p, measure, &text, theme::size::BODY, theme::TEXT_SECONDARY, false,
            Rect::new(SB_X + half, facts_y, half, body_h), HAlign::Right);
    }
}

fn draw_progress(p: Painter, a: &Airing, now: i64, measure: &dyn plx_machine::machine::Measure) {
    let start = plx_ui::fmt::wall_time(a.start_ms);
    let stop = plx_ui::fmt::wall_time(a.stop_ms);
    let cap_h = measure.line_h(theme::size::CAPTION);
    let label_w = measure.width_str(&start, theme::size::CAPTION, false).max(measure.width_str(&stop, theme::size::CAPTION, false)) + theme::space::SM;
    let bar = Rect::new(SB_X + label_w, SB_Y, sb_w() - 2.0 * label_w, SB_H);
    line(p, measure, &start, theme::size::CAPTION, theme::TEXT_SECONDARY, false,
        Rect::new(SB_X, SB_Y + SB_H * 0.5 - cap_h * 0.5, label_w, cap_h), HAlign::Left);
    line(p, measure, &stop, theme::size::CAPTION, theme::TEXT_SECONDARY, false,
        Rect::new(bar.x + bar.w, SB_Y + SB_H * 0.5 - cap_h * 0.5, label_w, cap_h), HAlign::Right);
    p.rect(bar, SB_H * 0.5, theme::RAIL_TRACK, theme::RAIL_TRACK, 0.0);
    let filled = bar.w * a.progress(now);
    if filled > 0.0 {
        p.rect(Rect::new(bar.x, bar.y, filled, bar.h), SB_H * 0.5, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
    }
}

/// How many channel rows the surf list shows at once (fewer on a shorter lineup).
pub const SURF_ROWS: usize = 7;
const SURF_W: f32 = 760.0;
/// The list's top edge: below the top margin, clear of the banner's first row (`SB_Y - 190`) for
/// [`SURF_ROWS`] rows at the shipped type sizes.
const SURF_TOP: f32 = plx_ui::consts::MARGIN_Y;
/// The airing's progress bar, under the row's two lines, and its air above.
const SURF_BAR_H: f32 = 3.0;
const SURF_BAR_GAP: f32 = 4.0;

/// The rows the surf list shows around `highlight`, in lineup order and wrapping like the channel
/// keys do: the highlight in the middle row when the lineup is long enough, each channel once.
pub fn surf_window(len: usize, highlight: usize) -> Vec<usize> {
    let n = len.min(SURF_ROWS);
    if n == 0 {
        return Vec::new();
    }
    let before = (n - 1) / 2;
    (0..n).map(|k| (highlight + len - before + k) % len).collect()
}

/// **The channel list over a playing channel** — UP/DOWN's surf. Each row is a channel's tile
/// (`plx_ui::channel_tile`: its logo with the number as a badge) and name, what it is airing now
/// and how far through; the highlight is the channel OK would
/// tune, and the channel already playing carries the guide's NOW mark at its left edge.
pub fn draw_surf(lineup: &Lineup, highlight: usize, playing: usize, now: i64, measure: &dyn plx_machine::machine::Measure) {
    let rows = surf_window(lineup.len(), highlight);
    if rows.is_empty() {
        return;
    }
    let p = Painter::root();
    let x = plx_ui::consts::SCR_W - plx_ui::consts::MARGIN_X - SURF_W;
    let pad = theme::space::XS;
    let body_h = measure.line_h(theme::size::BODY);
    let cap_h = measure.line_h(theme::size::CAPTION);
    // A row: its two lines, the bar a hairline under them, and the same air above and below.
    let row_h = body_h + cap_h + SURF_BAR_GAP + SURF_BAR_H + 2.0 * pad;
    let panel = Rect::new(x, SURF_TOP, SURF_W, rows.len() as f32 * row_h + 2.0 * pad);
    p.rect(panel, theme::space::MD, theme::scrim_black(0.72), theme::scrim_black(0.72), 0.0);
    // Each row leads with the channel's tile — its logo, or its initials, with the number as the
    // tile's badge — as tall as the row's two lines and 16:9, the guide's own channel column.
    let tile_h = body_h + cap_h;
    let tile_w = tile_h * 16.0 / 9.0;
    for (k, &i) in rows.iter().enumerate() {
        let Some(ch) = lineup.channels.get(i) else { continue };
        let row = Rect::new(x + pad, SURF_TOP + pad + k as f32 * row_h, SURF_W - 2.0 * pad, row_h);
        if i == highlight {
            p.rect(row, theme::space::SM, theme::OVERLAY_FOCUS_PILL, theme::OVERLAY_FOCUS_PILL, 0.0);
        }
        if i == playing {
            p.rect(Rect::new(row.x, row.y + theme::space::SM, 4.0, row.h - 2.0 * theme::space::SM), 2.0, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
        }
        let inner = Rect::new(row.x + theme::space::MD, row.y, row.w - 2.0 * theme::space::MD, row.h);
        let top = inner.y + pad;
        ChannelTile::new(&ch.number, &ch.name, &ch.icon).draw(p, Rect::new(inner.x, top, tile_w, tile_h), theme::space::XS, measure);
        let text_x = inner.x + tile_w + theme::space::SM;
        let text_w = inner.w - tile_w - theme::space::SM;
        line(p, measure, &ch.name, theme::size::BODY, theme::TEXT_PRIMARY, true,
            Rect::new(text_x, top, text_w, body_h), HAlign::Left);
        let (on_now, _) = ch.now_next(now);
        let airing = on_now.map(|a| a.title.as_str()).unwrap_or_else(|| plx_platform::i18n::msg::livetv_no_info());
        line(p, measure, airing, theme::size::CAPTION, theme::TEXT_SECONDARY, false,
            Rect::new(text_x, top + body_h, text_w, cap_h), HAlign::Left);
        if let Some(a) = on_now {
            let bar = Rect::new(text_x, top + body_h + cap_h + SURF_BAR_GAP, text_w, SURF_BAR_H);
            p.rect(bar, SURF_BAR_H * 0.5, theme::RAIL_TRACK, theme::RAIL_TRACK, 0.0);
            let filled = bar.w * a.progress(now);
            if filled > 0.0 {
                p.rect(Rect::new(bar.x, bar.y, filled, bar.h), SURF_BAR_H * 0.5, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
            }
        }
    }
}

/// The surf list's bottom edge for a row height — kept above the banner's first row
/// (`live_banner_rows_clear_the_surf_list` holds it at the fixture's metrics).
pub fn surf_bottom(row_h: f32, rows: usize) -> f32 {
    SURF_TOP + rows as f32 * row_h + 2.0 * theme::space::XS
}

/// The channel number being keyed in, top right.
pub fn draw_typed(typed: &str, measure: &dyn plx_machine::machine::Measure) {
    if typed.is_empty() {
        return;
    }
    let p = Painter::root();
    let h = measure.line_h(theme::size::DISPLAY);
    let w = measure.width_str(typed, theme::size::DISPLAY, true) + 2.0 * theme::space::MD;
    let r = Rect::new(plx_ui::consts::SCR_W - plx_ui::consts::MARGIN_X - w, plx_ui::consts::MARGIN_Y, w, h + theme::space::SM);
    p.rect(r, theme::space::XS, theme::scrim_black(0.6), theme::scrim_black(0.6), 0.0);
    line(p, measure, typed, theme::size::DISPLAY, theme::TEXT_PRIMARY, true, r, HAlign::Center);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surf_list_clears_the_banner() {
        use plx_machine::machine::Measure as _;
        let m = plx_ui::fixture::FixtureMeasure;
        let row_h = m.line_h(theme::size::BODY) + m.line_h(theme::size::CAPTION) + SURF_BAR_GAP + SURF_BAR_H
            + 2.0 * theme::space::XS;
        assert!(surf_bottom(row_h, SURF_ROWS) < SB_Y - 190.0, "the list ends above the LIVE badge row");
    }

    #[test]
    fn the_surf_window_centres_the_highlight_and_wraps_like_the_channel_keys() {
        assert_eq!(surf_window(20, 10), vec![7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(surf_window(20, 0), vec![17, 18, 19, 0, 1, 2, 3], "the top wraps to the end");
        assert_eq!(surf_window(3, 1), vec![0, 1, 2], "a short lineup shows each channel once");
        assert_eq!(surf_window(4, 0), vec![3, 0, 1, 2]);
        assert!(surf_window(0, 0).is_empty());
    }
}
