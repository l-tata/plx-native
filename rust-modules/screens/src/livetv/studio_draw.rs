//! **The channel studio's drawing** ([`super::studio`]): the full-bleed billboard of the focused
//! card — the picture of what it would air right now, its name, why it is suggested, its live
//! preview and its actions — over the two rows of cards. Layout only; every decision a test can
//! hold is in `studio`.

use super::draw::line;
use super::studio::{self, Act, Card, Studio, Zone, ROW_SUGGESTED, ROW_YOURS};
use super::*;
use plx_data::vchannel::schedule::Schedule;
use plx_ui::label::HAlign;
use plx_ui::text_view::TextView;
use plx_ui::widgets::TabPill;
use plx_ui::{Crop, Painter, View};

/// The cards: 16:9, six to a row.
pub const CARD_H: f32 = 140.0;
pub const CARD_W: f32 = CARD_H * 16.0 / 9.0;
pub const CARD_GAP: f32 = theme::space::MD;
const CARD_RAD: f32 = theme::CARD_RING_RAD;
/// A row: its heading, then its cards. The bottom row sits on the safe area's foot.
const ROW_LABEL_H: f32 = 36.0;
const ROW_PITCH: f32 = ROW_LABEL_H + CARD_H + theme::space::LG;
const ROWS_BOTTOM: f32 = SCR_H - MARGIN_Y;
/// The billboard's copy column and its action row.
const COPY_TOP: f32 = TOP + theme::space::SM;
const COPY_W: f32 = 1_040.0;
const ACT_H: f32 = 56.0;
const ACT_SZ: i32 = theme::size::LABEL;
/// The preview's progress bar.
const BAR_W: f32 = 520.0;
const BAR_H: f32 = 6.0;
/// The backdrop's request size, and how much of it shows through.
const ART_REQ: (i32, i32) = (960, 540);
const ART_A: f32 = 0.62;

/// How many cards a row shows.
pub fn per_row() -> usize {
    (((SCR_W - 2.0 * MARGIN_X + CARD_GAP) / (CARD_W + CARD_GAP)).floor() as usize).max(1)
}

/// The top of the cards of `row`, given which rows have cards: the rows stack up from the foot.
fn cards_y(row: usize, yours: bool) -> f32 {
    let from_bottom = if yours && row == ROW_SUGGESTED { 1 } else { 0 };
    ROWS_BOTTOM - CARD_H - from_bottom as f32 * ROW_PITCH
}

/// The first card a row shows, so its cursor is always on screen.
fn first(col: usize) -> usize {
    (col + 1).saturating_sub(per_row())
}

/// The rect of card `col` in `row`.
pub fn card_rect(st: &Studio, row: usize, col: usize, yours: bool) -> Rect {
    let x = MARGIN_X + (col as f32 - first(st.col[row]) as f32) * (CARD_W + CARD_GAP);
    Rect::new(x, cards_y(row, yours), CARD_W, CARD_H)
}

/// The top of the action row: above the top row's heading.
fn acts_y(yours: bool) -> f32 {
    let top_row = if yours { ROW_SUGGESTED } else { ROW_YOURS };
    cards_y(top_row, yours) - ROW_LABEL_H - theme::space::LG - ACT_H
}

/// A label of the focused card's action row.
fn act_label(st: &Studio, act: Act, card: &Card<'_>) -> String {
    use plx_platform::i18n::msg;
    match act {
        Act::Keep if st.keeping.is_some() => msg::livetv_studio_keeping().to_owned(),
        Act::Keep => msg::livetv_studio_keep().to_owned(),
        Act::Reshuffle => msg::livetv_studio_reshuffle().to_owned(),
        Act::Order => {
            let style = match card {
                Card::Channel(c) => c.recipe.style,
                _ => card.suggestion().and_then(|s| st.draft_of(s)).map(|r| r.style).or(card.suggestion().map(|s| s.style)).unwrap_or_default(),
            };
            msg::livetv_studio_order(studio::style_name(style))
        }
        Act::NotInterested => msg::livetv_studio_not_interested().to_owned(),
        Act::Surprise if matches!(card, Card::Surprise(Some(_))) => msg::livetv_studio_surprise_again().to_owned(),
        Act::Surprise => msg::livetv_studio_surprise().to_owned(),
        Act::Watch => msg::livetv_studio_watch().to_owned(),
        Act::Delete if matches!(card, Card::Channel(c) if st.confirm_delete.as_deref() == Some(c.playlist.as_str())) =>
            msg::livetv_studio_delete_confirm().to_owned(),
        Act::Delete => msg::livetv_studio_delete().to_owned(),
        Act::Edit => msg::livetv_studio_edit().to_owned(),
    }
}

/// The focused card's action pills, left to right.
/// While the options are open ([`Zone::Edit`]) the row is the options instead, each with whether
/// it is SELECTED (an option that is on).
pub fn act_rects(st: &Studio, card: &Card<'_>, yours: bool, measure: &dyn plx_machine::machine::Measure) -> Vec<(Rect, String)> {
    act_row(st, card, yours, measure).into_iter().map(|(r, label, _)| (r, label)).collect()
}

fn act_row(st: &Studio, card: &Card<'_>, yours: bool, measure: &dyn plx_machine::machine::Measure) -> Vec<(Rect, String, bool)> {
    let y = acts_y(yours);
    let mut x = MARGIN_X;
    let mut place = |label: String, on: bool| {
        let w = TabPill::width_measured(&label, ACT_SZ, measure);
        let r = Rect::new(x, y, w, ACT_H);
        x += w + theme::space::SM;
        (r, label, on)
    };
    if st.zone == Zone::Edit {
        if let Some(r) = card.suggestion().and_then(|s| st.draft_of(s)) {
            return studio::EDIT_ITEMS
                .iter()
                .map(|&item| {
                    let on = match item {
                        studio::EditItem::Unwatched => r.rules.unwatched_only,
                        studio::EditItem::Kinds => r.rules.kinds != plx_data::vchannel::recipe::Kinds::Both,
                        studio::EditItem::Rating => r.rules.max_rating_rank > 0,
                        studio::EditItem::Done => false,
                    };
                    place(studio::edit_label(item, r), on)
                })
                .collect();
        }
    }
    studio::acts(card).into_iter().map(|a| place(act_label(st, a, card), false)).collect()
}


/// The picture a card stands for: what is on it right now, else the suggestion's backdrop, else
/// the channel playlist's composite.
pub fn card_art<'a>(card: &Card<'a>, schedule: Option<&'a Schedule>, now: i64) -> Option<(u16, &'a str)> {
    if let Some(p) = schedule.and_then(|s| s.at(now).map(|slot| &s.programs()[slot.program])) {
        let path = if p.art.is_empty() { p.thumb.as_str() } else { p.art.as_str() };
        if !path.is_empty() {
            return Some((p.sid, path));
        }
    }
    match card {
        Card::Channel(c) if !c.composite.is_empty() => Some((plx_plex::plex::current_server().raw(), c.composite.as_str())),
        _ => card.suggestion().and_then(|s| s.art.as_ref()).map(|(sid, p)| (*sid, p.as_str())),
    }
}

fn texture(p: Painter, art: Option<(u16, &str)>, w: i32, h: i32) -> (u32, f32, f32) {
    match art {
        Some((sid, path)) if !p.is_recording() && !path.is_empty() => plx_ui::tex::resolve_wh_on(sid, path, w, h, false),
        _ => (0, 0.0, 0.0),
    }
}

impl LiveTvScreen {
    pub(super) fn draw_studio(&self, p: Painter, view: LiveTvView<'_>, focused: bool, tick_ms: u32, measure: &dyn plx_machine::machine::Measure) {
        let st = &self.studio;
        let rows = studio::rows(view);
        let yours = !rows[ROW_YOURS].is_empty();
        let now = plx_base::wallclock::now_ms();
        let card = st.focus(&rows);
        if card.is_none() {
            let v = view.virtuals();
            let (text, kind) = if v.busy() || v.catalog().is_none() {
                (plx_platform::i18n::msg::livetv_studio_loading(), StatusKind::Working)
            } else {
                (plx_platform::i18n::msg::livetv_studio_empty(), StatusKind::Empty)
            };
            let caption = std::ffi::CString::new(text).unwrap_or_default();
            // placeholder-exempt: builds the value only; counted where drawn (StatusOverlay)
            StatusOverlay::new(guide_frame(), &caption, kind).phase(tick_ms).draw_measured(&plx_ui::Env::inert(), p, measure);
            return;
        }
        let card = card.unwrap_or(Card::Surprise(None));
        let schedule = st.schedule(&card, view);
        // The billboard's picture, full bleed, darkened where the copy and the rows stand.
        let (t, tw, th) = texture(p, card_art(&card, schedule, now), ART_REQ.0, ART_REQ.1);
        if t != 0 && th > 0.0 {
            p.tex_uv(t, Rect::FULL.cover_uv(tw, th, Crop::Centre), Rect::FULL, 0.0, [1.0, 1.0, 1.0, ART_A]);
        }
        plx_ui::widgets::hero_scrim(p, 1.0, false);
        let ramp_top = acts_y(yours) - theme::space::LG;
        let clear = theme::scrim(0.0);
        let deep = theme::scrim(0.88);
        p.grad4(Rect::new(0.0, ramp_top, SCR_W, SCR_H - ramp_top), [clear, clear, deep, deep]);

        self.draw_copy(p, view, &card, schedule, now, measure);
        let cursor = if st.zone == Zone::Edit { st.edit } else { st.act };
        for (i, (r, label, on)) in act_row(st, &card, yours, measure).iter().enumerate() {
            let label = std::ffi::CString::new(label.as_str()).unwrap_or_default();
            TabPill::new(label.as_ptr(), ACT_SZ, *r)
                .focused(focused && st.zone != Zone::Cards && i == cursor)
                .selected(*on)
                .plated()
                .draw(&plx_ui::Env::inert(), p);
        }
        for (row, cards) in rows.iter().enumerate() {
            if cards.is_empty() {
                continue;
            }
            let y = cards_y(row, yours);
            let heading = if row == ROW_SUGGESTED {
                plx_platform::i18n::msg::livetv_studio_suggested()
            } else {
                plx_platform::i18n::msg::livetv_studio_yours()
            };
            line(p, measure, heading, theme::size::HEADLINE, theme::TEXT_PRIMARY, true,
                Rect::new(MARGIN_X, y - ROW_LABEL_H - theme::space::XS, COPY_W, ROW_LABEL_H), HAlign::Left);
            let from = first(st.col[row]);
            for (col, c) in cards.iter().enumerate().skip(from).take(per_row()) {
                let on = focused && st.zone == Zone::Cards && st.row == row && st.col[row] == col;
                let sel = st.row == row && st.col[row] == col;
                self.draw_card(p, view, c, card_rect(st, row, col, yours), on, sel, now, measure);
            }
        }
        if let Some((text, _)) = &st.toast {
            let w = measure.width_str(text, theme::size::LABEL, true) + 2.0 * theme::space::MD;
            let r = Rect::new(SCR_W - MARGIN_X - w, TOP, w, ACT_H);
            p.rect(r, r.h * 0.5, theme::ACCENT, theme::ACCENT, 0.0);
            line(p, measure, text, theme::size::LABEL, theme::ACCENT_INK, true, r, HAlign::Center);
        }
    }

    /// The billboard's copy: what the card is, its name, why, what it holds, and its preview.
    fn draw_copy(&self, p: Painter, view: LiveTvView<'_>, card: &Card<'_>, schedule: Option<&Schedule>, now: i64, measure: &dyn plx_machine::machine::Measure) {
        use plx_platform::i18n::msg;
        let x = MARGIN_X;
        let mut y = COPY_TOP;
        let (over, name, why, facts): (String, &str, &str, String) = match card {
            Card::Idea(s) => (msg::livetv_studio_suggested().to_owned(), &s.name, &s.why, self.studio.estimate_of(s).map_or_else(|| s.tagline.clone(), studio::estimate_line)),
            Card::Surprise(Some(s)) => (msg::livetv_studio_surprise().to_owned(), &s.name, &s.why, self.studio.estimate_of(s).map_or_else(|| s.tagline.clone(), studio::estimate_line)),
            Card::Surprise(None) => (String::new(), msg::livetv_studio_surprise(), msg::livetv_studio_surprise_body(), String::new()),
            Card::Channel(c) => (msg::livetv_studio_number(&c.number()), &c.recipe.name, &c.recipe.tagline, String::new()),
        };
        let cap = measure.line_h(theme::size::CAPTION);
        if !over.is_empty() {
            line(p, measure, &over.to_uppercase(), theme::size::CAPTION, theme::TEXT_SECONDARY, true, Rect::new(x, y, COPY_W, cap), HAlign::Left);
        }
        y += cap;
        let name_h = measure.line_h(theme::size::DISPLAY);
        line(p, measure, name, theme::size::DISPLAY, theme::TEXT_PRIMARY, true, Rect::new(x, y, COPY_W, name_h), HAlign::Left);
        y += name_h;
        if !why.is_empty() {
            let body = TextView::new(why, theme::size::BODY, theme::TEXT_READING).with_measure(measure).max_lines(2);
            let h = body.measure_h(COPY_W);
            body.draw(p, Rect::new(x, y, COPY_W, h));
            y += h;
        }
        let label_h = measure.line_h(theme::size::LABEL);
        if !facts.is_empty() {
            line(p, measure, &facts, theme::size::LABEL, theme::TEXT_TERTIARY, false, Rect::new(x, y, COPY_W, label_h), HAlign::Left);
            y += label_h;
        }
        y += theme::space::SM;
        if matches!(card, Card::Surprise(None)) {
            return;
        }
        let Some(s) = schedule.filter(|s| !s.is_empty()) else {
            let text = if self.studio.unbuildable(card, view) || matches!(card, Card::Channel(c) if c.failed) {
                msg::livetv_studio_unbuildable()
            } else {
                msg::livetv_studio_building()
            };
            line(p, measure, text, theme::size::LABEL, theme::TEXT_SECONDARY, false, Rect::new(x, y, COPY_W, label_h), HAlign::Left);
            return;
        };
        let Some(slot) = s.at(now) else { return };
        let prog = &s.programs()[slot.program];
        // On now: the programme, how far in, how long is left.
        line(p, measure, &msg::livetv_studio_on_now().to_uppercase(), theme::size::CAPTION, theme::GUIDE_NOW, true,
            Rect::new(x, y, COPY_W, cap), HAlign::Left);
        y += cap;
        let head_h = measure.line_h(theme::size::HEADLINE);
        line(p, measure, &studio::programme_line(prog), theme::size::HEADLINE, theme::TEXT_PRIMARY, true,
            Rect::new(x, y, COPY_W, head_h), HAlign::Left);
        y += head_h + theme::space::XS;
        let len = (slot.stop_ms - slot.start_ms).max(1);
        let frac = ((now - slot.start_ms) as f32 / len as f32).clamp(0.0, 1.0);
        let bar = Rect::new(x, y + (label_h - BAR_H) * 0.5, BAR_W, BAR_H);
        p.rect(bar, BAR_H * 0.5, theme::GUIDE_CELL, theme::GUIDE_CELL, 0.0);
        p.rect(Rect::new(bar.x, bar.y, (BAR_W * frac).max(BAR_H), BAR_H), BAR_H * 0.5, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
        line(p, measure, &plx_ui::fmt::time_left(slot.stop_ms - now), theme::size::LABEL, theme::TEXT_SECONDARY, false,
            Rect::new(bar.x + BAR_W + theme::space::SM, y, COPY_W - BAR_W - theme::space::SM, label_h), HAlign::Left);
        y += label_h + theme::space::XS;
        // Up next: the next few, with their start times.
        let mut next = s.after(&slot);
        let floor = acts_y(!studio::rows(view)[ROW_YOURS].is_empty()) - theme::space::SM;
        for i in 0..studio::UP_NEXT {
            let Some(n) = next else { break };
            if y + label_h > floor {
                break;
            }
            let lead = if i == 0 { format!("{} \u{b7} ", msg::livetv_studio_up_next()) } else { String::new() };
            let text = format!("{lead}{}  {}", plx_ui::fmt::wall_time(n.start_ms), studio::programme_line(&s.programs()[n.program]));
            line(p, measure, &text, theme::size::LABEL, theme::TEXT_SECONDARY, false, Rect::new(x, y, COPY_W, label_h), HAlign::Left);
            y += label_h;
            next = s.after(&n);
        }
    }

    /// One card: its picture under a foot of shade carrying its name; a kept channel's number in
    /// the corner; *Surprise me* as a plain plate.
    #[allow(clippy::too_many_arguments)]
    fn draw_card(&self, p: Painter, view: LiveTvView<'_>, card: &Card<'_>, r: Rect, on: bool, sel: bool, now: i64, measure: &dyn plx_machine::machine::Measure) {
        let schedule = self.studio.schedule(card, view);
        let (t, tw, th) = texture(p, card_art(card, schedule, now), CARD_W as i32 * 2, CARD_H as i32 * 2);
        let f = if on { 1.0 } else { 0.0 };
        if t != 0 && th > 0.0 {
            p.tex_carded(t, r.cover_uv(tw, th, Crop::Centre), r, CARD_RAD, theme::TINT_WHITE, f);
        } else {
            let (top, bot) = if matches!(card, Card::Surprise(_)) { (theme::GUIDE_CELL_NOW, theme::GUIDE_CELL) } else { (theme::GUIDE_CELL, theme::GUIDE_CELL) };
            p.focus_shadow(r, CARD_RAD, f);
            p.rect(r, CARD_RAD, top, bot, 0.0);
        }
        let foot = r.h * 0.55;
        p.grad4(Rect::new(r.x, r.y + r.h - foot, r.w, foot), [theme::scrim(0.0), theme::scrim(0.0), theme::scrim(0.8), theme::scrim(0.8)]);
        let name: &str = match card {
            Card::Idea(s) => &s.name,
            Card::Surprise(Some(s)) => &s.name,
            Card::Surprise(None) => plx_platform::i18n::msg::livetv_studio_surprise(),
            Card::Channel(c) => &c.recipe.name,
        };
        let pad = theme::space::SM;
        let lh = measure.line_h(theme::size::LABEL);
        line(p, measure, name, theme::size::LABEL, theme::TEXT_PRIMARY, true,
            Rect::new(r.x + pad, r.y + r.h - pad - lh, r.w - 2.0 * pad, lh), HAlign::Left);
        if let Card::Channel(c) = card {
            let num = c.number();
            let w = measure.width_str(&num, theme::size::MICRO, true) + 2.0 * theme::space::XS;
            let badge = Rect::new(r.x + theme::space::XS, r.y + theme::space::XS, w, measure.line_h(theme::size::MICRO));
            p.rect(badge, theme::space::XS, theme::scrim(0.7), theme::scrim(0.7), 0.0);
            line(p, measure, &num, theme::size::MICRO, theme::TEXT_PRIMARY, true, badge, HAlign::Center);
        }
        if on {
            p.rring(r, CARD_RAD, 3.0, theme::TEXT_PRIMARY);
        } else if sel {
            p.rring(r, CARD_RAD, 2.0, theme::TEXT_TERTIARY);
        }
    }

    /// Where the engine places focus on the studio: the focused card, or its focused action.
    pub(super) fn studio_focus_rect(&self, view: LiveTvView<'_>, measure: &dyn plx_machine::machine::Measure) -> Rect {
        let st = &self.studio;
        let rows = studio::rows(view);
        let yours = !rows[ROW_YOURS].is_empty();
        let Some(card) = st.focus(&rows) else { return guide_frame() };
        match st.zone {
            Zone::Cards => card_rect(st, st.row, st.col[st.row], yours),
            Zone::Actions => act_rects(st, &card, yours, measure).get(st.act).map(|(r, _)| *r).unwrap_or_else(guide_frame),
            Zone::Edit => act_rects(st, &card, yours, measure).get(st.edit).map(|(r, _)| *r).unwrap_or_else(guide_frame),
        }
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn two_rows_and_the_actions_fit_under_the_copy() {
        assert!(per_row() >= 5, "{}", per_row());
        let st = Studio::default();
        let last = card_rect(&st, ROW_SUGGESTED, per_row() - 1, true);
        assert!(last.x + last.w <= SCR_W - MARGIN_X + 0.5, "the last card stays in the safe area");
        assert!(cards_y(ROW_YOURS, true) + CARD_H <= SCR_H - MARGIN_Y + 0.5);
        // The name, two lines of why, a facts line, the on-now block and one up-next line clear the
        // action row.
        let copy = theme::size::CAPTION as f32 * 1.4 + theme::size::DISPLAY as f32 * 1.3 + 2.0 * theme::size::BODY as f32 * 1.4
            + theme::size::LABEL as f32 * 1.4 * 3.0 + theme::size::HEADLINE as f32 * 1.3 + theme::size::CAPTION as f32 * 1.4;
        assert!(COPY_TOP + copy <= acts_y(true), "copy ends at {} under actions at {}", COPY_TOP + copy, acts_y(true));
    }
}
