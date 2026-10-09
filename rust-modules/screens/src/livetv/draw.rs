//! **The Live TV page's drawing** — the guide (info pane, genre strip, time axis, channel rows)
//! and the setup face. Layout only: what is shown and where the cursor is are the page's
//! (`super`), and every rule a test can hold — the strip's chips, the rows a filter keeps, the
//! timing words, a cell's genre edge — is in [`super::filter`].

use super::*;
use plx_ui::channel_tile::ChannelTile;
use plx_ui::label::{HAlign, Label};
use plx_ui::text_view::TextView;
use plx_ui::widgets::{KeyHint, TabPill};
use plx_ui::{Crop, Painter, View};
use std::ffi::CStr;
use std::rc::Rc;

/// The chips' type rung.
const CHIP_SZ: i32 = theme::size::CAPTION;
/// The NOW marker's time label: its height inside the axis band and its padding.
const NOW_LABEL_H: f32 = 30.0;
const NOW_LABEL_PAD: f32 = theme::space::XS;
/// The coloured dot before the info pane's category.
const DOT_D: f32 = 12.0;

/// One line of text in `frame`, cut to fit with an ellipsis.
pub(super) fn line(p: Painter, measure: &dyn plx_machine::machine::Measure, text: &str, sz: i32, col: [f32; 4], bold: bool, frame: Rect, h: HAlign) {
    if text.is_empty() || frame.w <= 0.0 {
        return;
    }
    let fitted = measure.fit_line(text, frame.w, sz, bold);
    let label = Label::new(fitted.as_ptr(), sz, col).h(h);
    if bold { label.bold().draw(p, frame) } else { label.draw(p, frame) };
}

/// A chip's label as drawn: a genre's own word whole, a guide category cut at [`CHIP_LABEL_MAX`].
fn chip_label(f: &Filter, measure: &dyn plx_machine::machine::Measure) -> Rc<CStr> {
    measure.fit_line(&f.label(), CHIP_LABEL_MAX, CHIP_SZ, true)
}

/// **The genre strip's chip rects**, left to right from the margin, for as many chips as fit the
/// row — the strip OFFERS only what it can draw, so the cursor never walks onto a chip off the
/// edge ([`strip_fit`]).
pub(super) fn strip_rects(chips: &[Filter], measure: &dyn plx_machine::machine::Measure) -> Vec<Rect> {
    let mut out = Vec::with_capacity(chips.len());
    let mut x = MARGIN_X;
    // The studio's pill stands at the strip's right end; the chips fill what is left of it.
    let end = studio_pill_rect(measure).x - CHIP_GAP;
    for chip in chips {
        let label = chip_label(chip, measure);
        let w = TabPill::width_measured(label.to_str().unwrap_or(""), CHIP_SZ, measure);
        if x + w > end {
            break;
        }
        out.push(Rect::new(x, STRIP_TOP, w, STRIP_H));
        x += w + CHIP_GAP;
    }
    out
}

/// The channel studio's pill, at the right end of the genre strip.
pub(super) fn studio_pill_rect(measure: &dyn plx_machine::machine::Measure) -> Rect {
    let w = TabPill::width_measured(plx_platform::i18n::msg::livetv_studio_title(), CHIP_SZ, measure);
    Rect::new(SCR_W - MARGIN_X - w, STRIP_TOP, w, STRIP_H)
}

/// How many of `chips` the strip shows.
pub(super) fn strip_fit(chips: &[Filter], measure: &dyn plx_machine::machine::Measure) -> usize {
    strip_rects(chips, measure).len()
}

/// The picture the info pane shows for `airing` on `ch`: the programme's own artwork, else the
/// channel's logo, else none. The page ground is keyed from the same picture.
pub(super) fn info_art<'a>(ch: &'a Channel, airing: Option<&'a Airing>) -> Option<&'a str> {
    airing.map(|a| a.icon.as_str()).filter(|u| !u.is_empty()).or(Some(ch.icon.as_str()).filter(|u| !u.is_empty()))
}

/// The episode line: `S02E05 · Episode title`, either half alone, or nothing.
pub(super) fn episode_line(a: &Airing) -> String {
    [a.episode.as_str(), a.sub_title.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" \u{b7} ")
}

impl LiveTvScreen {
    /// The rect of the guide cell under the cursor (or of the focused row's band when the row has
    /// nothing listed there) — what the engine places focus at.
    pub(super) fn focus_cell_rect(&self, view: LiveTvView<'_>) -> Rect {
        let c = &self.cursor;
        let y = row_y(c.row, c.top);
        let window_end = c.window_ms + grid::WINDOW_MS;
        let cell = self
            .focus_airing(view.lineup())
            .and_then(|(_, a)| a)
            .map(|a| (a.start_ms.max(c.window_ms), a.stop_ms.min(window_end)));
        match cell {
            Some((x0, x1)) => {
                let (l, r) = (x_of(x0, c.window_ms), x_of(x1, c.window_ms));
                Rect::new(l + CELL_GAP * 0.5, y, (r - l - CELL_GAP).max(1.0), ROW_H - ROW_GAP)
            }
            None => Rect::new(CELLS_X, y, CELLS_W, ROW_H - ROW_GAP),
        }
    }

    pub(super) fn draw_guide(&self, p: Painter, view: LiveTvView<'_>, focused: bool, measure: &dyn plx_machine::machine::Measure) {
        let lineup = view.lineup();
        if lineup.is_empty() {
            StatusOverlay::new(guide_frame(), plx_platform::i18n::msg::livetv_empty_c(), StatusKind::Empty)
                .draw_measured(&plx_ui::Env::inert(), p, measure);
            return;
        }
        let now = plx_base::wallclock::now_ms();
        self.draw_info(p, view, measure);
        self.draw_strip(p, focused, measure);
        let rows = Rows::some(lineup, &self.rows);
        if rows.is_empty() {
            let text = plx_platform::i18n::msg::livetv_filter_empty(&self.filter.label());
            let caption = std::ffi::CString::new(text).unwrap_or_default();
            let frame = Rect::new(MARGIN_X, AXIS_TOP, SCR_W - 2.0 * MARGIN_X, SCR_H - MARGIN_Y - AXIS_TOP);
            StatusOverlay::new(frame, &caption, StatusKind::Empty).draw_measured(&plx_ui::Env::inert(), p, measure);
            return;
        }
        let c = self.cursor;
        let window_end = c.window_ms + grid::WINDOW_MS;
        let visible = visible_rows();
        let shown = (c.top..(c.top + visible).min(rows.len())).len();
        self.draw_axis(p, now, measure);
        if !self.typed.is_empty() {
            line(p, measure, &self.typed, theme::size::HEADLINE, theme::TEXT_PRIMARY, true,
                Rect::new(MARGIN_X, AXIS_TOP, CH_W, AXIS_H), HAlign::Left);
        }
        // NOW: the line down the rows, from under its time label — drawn BEFORE the rows, so it
        // shows through the translucent plates but never through a title, and the focused cell's
        // opaque accent covers it.
        if now >= c.window_ms && now < window_end {
            let x = x_of(now, c.window_ms);
            let top = AXIS_TOP + (AXIS_H + NOW_LABEL_H) * 0.5;
            let rows_h = shown as f32 * ROW_H - ROW_GAP;
            p.rect(Rect::new(x - NOW_W * 0.5, top, NOW_W, ROWS_TOP + rows_h - top), NOW_W * 0.5, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
        }
        let on_grid = focused && !self.on_strip;
        for row in c.top..(c.top + visible).min(rows.len()) {
            let Some(ch) = rows.get(row) else { continue };
            let y = row_y(row, c.top);
            let on_row = on_grid && row == c.row;
            let band = ROW_H - ROW_GAP;
            // The channel column: its logo tile, then its name.
            let chr = Rect::new(MARGIN_X, y, CH_W, band);
            let fill = if on_row { theme::GUIDE_CELL_NOW } else { theme::GUIDE_CELL };
            p.rect(chr, CELL_RAD, fill, fill, 0.0);
            ChannelTile::new(&ch.number, &ch.name, &ch.icon).draw(p, Rect::new(chr.x, y, TILE_W, band), CELL_RAD, measure);
            let name_x = chr.x + TILE_W + theme::space::SM;
            let name_ink = if on_row { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
            line(p, measure, &ch.name, theme::size::LABEL, name_ink, on_row,
                Rect::new(name_x, y, chr.x + chr.w - theme::space::SM - name_x, band), HAlign::Left);
            // The airings in the window.
            let mut any = false;
            for a in ch.airings.iter().filter(|a| a.stop_ms > c.window_ms && a.start_ms < window_end) {
                any = true;
                let (l, r) = (x_of(a.start_ms.max(c.window_ms), c.window_ms), x_of(a.stop_ms.min(window_end), c.window_ms));
                let cell = Rect::new(l + CELL_GAP * 0.5, y, (r - l - CELL_GAP).max(1.0), band);
                let is_focus = on_row && a.covers(c.at_ms);
                self.draw_cell(p, a, cell, is_focus, now, c.window_ms, measure);
            }
            if !any {
                let cell = Rect::new(CELLS_X, y, CELLS_W, band);
                let (fill, ink) = if on_row { (theme::ACCENT, theme::ACCENT_INK) } else { (theme::GUIDE_CELL, theme::TEXT_TERTIARY) };
                p.rect(cell, CELL_RAD, fill, fill, 0.0);
                line(p, measure, plx_platform::i18n::msg::livetv_no_info(), theme::size::LABEL, ink, false,
                    Rect::new(cell.x + theme::space::SM, y, cell.w - 2.0 * theme::space::SM, band), HAlign::Left);
            }
        }
    }

    /// One programme cell: its plate (the accent under the cursor, a brighter rung while on now,
    /// the elapsed part washed), its genre edge, its title and its sub-line — how long is left for
    /// an airing on now, its time span otherwise. A cell the genre filter does not keep is dimmed,
    /// unless the cursor is on it.
    #[allow(clippy::too_many_arguments)]
    fn draw_cell(&self, p: Painter, a: &Airing, cell: Rect, is_focus: bool, now: i64, window: i64, measure: &dyn plx_machine::machine::Measure) {
        let p = if is_focus || self.filter.matches(a) { p } else { p.alpha(filter::DIM) };
        let on_now = a.covers(now);
        let (fill, ink, sub) = if is_focus {
            (theme::ACCENT, theme::ACCENT_INK, theme::ACCENT_INK)
        } else if on_now {
            (theme::GUIDE_CELL_NOW, theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
        } else {
            (theme::GUIDE_CELL, theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
        };
        p.rect(cell, CELL_RAD, fill, fill, 0.0);
        if on_now && !is_focus {
            let elapsed = (x_of(now, window) - cell.x).clamp(0.0, cell.w);
            if elapsed > 0.0 {
                p.rect(Rect::new(cell.x, cell.y, elapsed, cell.h), CELL_RAD, theme::GUIDE_CELL_ELAPSED, theme::GUIDE_CELL_ELAPSED, 0.0);
            }
        }
        if let Some(edge) = filter::genre_edge(a).filter(|_| cell.w > 4.0 * EDGE_W) {
            let bar = Rect::new(cell.x + (theme::space::SM - EDGE_W) * 0.5, cell.y + EDGE_INSET, EDGE_W, cell.h - 2.0 * EDGE_INSET);
            p.rect(bar, EDGE_W * 0.5, edge, edge, 0.0);
        }
        let text_x = cell.x + theme::space::SM + EDGE_W;
        let text_w = cell.x + cell.w - theme::space::SM - text_x;
        let half = cell.h * 0.5;
        let title = if a.title.is_empty() { plx_platform::i18n::msg::livetv_no_info() } else { a.title.as_str() };
        line(p, measure, title, theme::size::LABEL, ink, true,
            Rect::new(text_x, cell.y + theme::space::XS, text_w, half - theme::space::XS), HAlign::Left);
        let sub_text = match filter::Timing::of(a, now) {
            t @ filter::Timing::Left(_) => t.text().unwrap_or_default(),
            _ => format!("{} – {}", plx_ui::fmt::wall_time(a.start_ms), plx_ui::fmt::wall_time(a.stop_ms)),
        };
        line(p, measure, &sub_text, theme::size::MICRO, sub, false,
            Rect::new(text_x, cell.y + half, text_w, half - theme::space::XS), HAlign::Left);
    }

    /// The time axis: a label every half hour, and the NOW marker's own label — the wall-clock
    /// time on the marker's amber, standing where the line starts. A half-hour label the NOW label
    /// would cover is left out. Drawn once, above the rows: paging never moves it.
    fn draw_axis(&self, p: Painter, now: i64, measure: &dyn plx_machine::machine::Measure) {
        let c = self.cursor;
        let window_end = c.window_ms + grid::WINDOW_MS;
        let now_label = (now >= c.window_ms && now < window_end).then(|| {
            let text = plx_ui::fmt::wall_time(now);
            let w = measure.width_str(&text, theme::size::CAPTION, true) + 2.0 * NOW_LABEL_PAD;
            let x = (x_of(now, c.window_ms) - w * 0.5).clamp(CELLS_X, CELLS_X + CELLS_W - w);
            (text, Rect::new(x, AXIS_TOP + (AXIS_H - NOW_LABEL_H) * 0.5, w, NOW_LABEL_H))
        });
        let slot_w = grid::SLOT_MS as f32 / grid::WINDOW_MS as f32 * CELLS_W;
        for k in 0..(grid::WINDOW_MS / grid::SLOT_MS) {
            let t = c.window_ms + k * grid::SLOT_MS;
            let text = plx_ui::fmt::wall_time(t);
            let x = x_of(t, c.window_ms) + theme::space::XS;
            let w = measure.width_str(&text, theme::size::CAPTION, false);
            if let Some((_, r)) = &now_label {
                if x < r.x + r.w + theme::space::XS && x + w > r.x - theme::space::XS {
                    continue;
                }
            }
            line(p, measure, &text, theme::size::CAPTION, theme::TEXT_TERTIARY, false,
                Rect::new(x, AXIS_TOP, slot_w - theme::space::XS, AXIS_H), HAlign::Left);
        }
        if let Some((text, r)) = now_label {
            p.rect(r, r.h * 0.5, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
            line(p, measure, &text, theme::size::CAPTION, theme::GUIDE_NOW_INK, true, r, HAlign::Center);
        }
    }

    /// The genre strip: one pill per chip that fits, the filter in force SELECTED, the chip under
    /// the cursor FOCUSED while the cursor is on the strip.
    fn draw_strip(&self, p: Painter, focused: bool, measure: &dyn plx_machine::machine::Measure) {
        for (i, (chip, r)) in self.chips.iter().zip(strip_rects(&self.chips, measure)).enumerate() {
            let label = chip_label(chip, measure);
            TabPill::new(label.as_ptr(), CHIP_SZ, r)
                .focused(focused && self.on_strip && i == self.strip_sel)
                .selected(*chip == self.filter)
                .draw(&plx_ui::Env::inert(), p);
        }
        let label = std::ffi::CString::new(plx_platform::i18n::msg::livetv_studio_title()).unwrap_or_default();
        TabPill::new(label.as_ptr(), CHIP_SZ, studio_pill_rect(measure))
            .focused(focused && self.on_strip && self.strip_sel == strip_fit(&self.chips, measure))
            .plated()
            .draw(&plx_ui::Env::inert(), p);
    }

    /// The info pane: the focused airing's artwork on the right (its programme picture, else its
    /// channel's tile), with the Plex hint under it when the airing is in the viewer's library; and
    /// on the left its channel, title, episode, times with how long is left, category, and two or
    /// three lines of description.
    fn draw_info(&self, p: Painter, view: LiveTvView<'_>, measure: &dyn plx_machine::machine::Measure) {
        let Some((ch, airing)) = self.focus_airing(view.lineup()) else { return };
        // The artwork, right-aligned so its width (a poster, a still) never moves the text.
        let right = SCR_W - MARGIN_X;
        let mut drew = false;
        if let Some(icon) = airing.map(|a| a.icon.as_str()).filter(|u| !u.is_empty()) {
            let (t, tw, th) = if p.is_recording() {
                (0, 0.0, 0.0)
            } else {
                { let (srv, path) = plx_ui::tex::art_source(icon); plx_ui::tex::resolve_wh_on(srv, path, ART_MAX_W as i32, ART_H as i32, false) }
            };
            if t != 0 && th > 0.0 {
                let w = ART_H * (tw / th).clamp(2.0 / 3.0, 16.0 / 9.0);
                let r = Rect::new(right - w, TOP, w, ART_H);
                p.tex_carded(t, r.cover_uv(tw, th, Crop::Centre), r, theme::CARD_RING_RAD, theme::TINT_WHITE, 0.0);
                drew = true;
            }
        }
        if !drew {
            ChannelTile::new(&ch.number, &ch.name, &ch.icon)
                .draw(p, Rect::new(right - ART_MAX_W, TOP, ART_MAX_W, ART_H), theme::CARD_RING_RAD, measure);
        }
        if self.focus_match(view).is_some() {
            let hint = KeyHint::translated(plx_platform::i18n::msg::livetv_plex_hint("\u{fffc}"), c"OK");
            let cy = TOP + ART_H + theme::space::XS + KeyHint::height() * 0.5;
            hint.draw(p, right - hint.width(measure), cy, measure);
        }
        // The text column.
        let x = MARGIN_X;
        let bottom = TOP + INFO_H;
        let mut y = TOP;
        let cap = measure.line_h(theme::size::CAPTION);
        line(p, measure, &format!("{} \u{b7} {}", ch.number, ch.name), theme::size::CAPTION, theme::TEXT_SECONDARY, false,
            Rect::new(x, y, INFO_W, cap), HAlign::Left);
        y += cap;
        let title_h = measure.line_h(theme::size::TITLE);
        let title = airing.map(|a| a.title.as_str()).filter(|t| !t.is_empty()).unwrap_or(plx_platform::i18n::msg::livetv_no_info());
        line(p, measure, title, theme::size::TITLE, theme::TEXT_PRIMARY, true, Rect::new(x, y, INFO_W, title_h), HAlign::Left);
        y += title_h;
        let Some(a) = airing else {
            if view.guide_error().is_some() {
                let body_h = measure.line_h(theme::size::BODY);
                line(p, measure, plx_platform::i18n::msg::livetv_guide_missing(), theme::size::BODY, theme::TEXT_SECONDARY, false,
                    Rect::new(x, y, INFO_W, body_h), HAlign::Left);
            }
            return;
        };
        let episode = episode_line(a);
        if !episode.is_empty() {
            let body_h = measure.line_h(theme::size::BODY);
            line(p, measure, &episode, theme::size::BODY, theme::TEXT_PRIMARY, false, Rect::new(x, y, INFO_W, body_h), HAlign::Left);
            y += body_h;
        }
        // Times, how long is left (or until it starts), and the category behind its genre's dot.
        let now = plx_base::wallclock::now_ms();
        let mut facts = format!("{} – {}", plx_ui::fmt::wall_time(a.start_ms), plx_ui::fmt::wall_time(a.stop_ms));
        if let Some(t) = filter::Timing::of(a, now).text() {
            facts.push_str(" \u{b7} ");
            facts.push_str(&t);
        }
        let label_h = measure.line_h(theme::size::LABEL);
        line(p, measure, &facts, theme::size::LABEL, theme::TEXT_SECONDARY, false, Rect::new(x, y, INFO_W, label_h), HAlign::Left);
        if !a.category.is_empty() {
            let mut cx = x + measure.width_str(&facts, theme::size::LABEL, false) + theme::space::MD;
            if let Some(edge) = filter::genre_edge(a) {
                p.rect(Rect::new(cx, y + (label_h - DOT_D) * 0.5, DOT_D, DOT_D), DOT_D * 0.5, edge, edge, 0.0);
                cx += DOT_D + theme::space::XS;
            }
            line(p, measure, &a.category, theme::size::LABEL, theme::TEXT_SECONDARY, false,
                Rect::new(cx, y, (x + INFO_W - cx).max(0.0), label_h), HAlign::Left);
        }
        y += label_h + theme::space::XS;
        if !a.desc.is_empty() && bottom > y {
            let lines = if episode.is_empty() { 3 } else { 2 };
            let desc = TextView::new(&a.desc, theme::size::LABEL, theme::TEXT_READING).with_measure(measure).max_lines(lines);
            let h = desc.measure_h(INFO_W).min(bottom - y);
            desc.draw(p, Rect::new(x, y, INFO_W, h));
        }
    }

    pub(super) fn draw_setup(&self, p: Painter, view: LiveTvView<'_>, focused: bool, tick_ms: u32, measure: &dyn plx_machine::machine::Measure) {
        let x = MARGIN_X;
        let mut y = TOP + theme::space::MD;
        let title_h = measure.line_h(theme::size::TITLE);
        line(p, measure, plx_platform::i18n::msg::livetv_setup_title(), theme::size::TITLE, theme::TEXT_PRIMARY, true,
            Rect::new(x, y, INFO_W, title_h), HAlign::Left);
        y += title_h + theme::space::SM;
        let body = TextView::new(plx_platform::i18n::msg::livetv_setup_body(), theme::size::BODY, theme::TEXT_READING).with_measure(measure).max_lines(3);
        let body_h = body.measure_h(1_100.0);
        body.draw(p, Rect::new(x, y, 1_100.0, body_h));
        y += body_h + theme::space::SM;
        let note: Option<String> = if let Status::Failed(_) = view.status() {
            Some(plx_platform::i18n::msg::livetv_setup_failed(view.source()))
        } else if matches!(view.discovery(), Discovery::Done(found) if found.is_empty()) {
            Some(plx_platform::i18n::msg::livetv_setup_none_found().to_owned())
        } else {
            None
        };
        if let Some(note) = note {
            line(p, measure, &note, theme::size::BODY, theme::TEXT_SECONDARY, false,
                Rect::new(x, y, INFO_W, measure.line_h(theme::size::BODY)), HAlign::Left);
        }
        let items = setup_items(view, self.force_setup);
        let sel = self.setup_sel.min(items.len().saturating_sub(1));
        for (i, item) in items.iter().enumerate() {
            let r = setup_row_rect(i);
            let on = focused && i == sel;
            match item {
                Item::Address => self.draw_field(p, r, on, tick_ms, measure),
                _ => {
                    let text: String = match item {
                        Item::Search if matches!(view.discovery(), Discovery::Searching) => plx_platform::i18n::msg::livetv_setup_searching().to_owned(),
                        Item::Search => plx_platform::i18n::msg::livetv_setup_search().to_owned(),
                        Item::Found(k) => match view.discovery() {
                            Discovery::Done(found) => found.get(*k).map(|f| format!("{} \u{b7} {}", f.name, f.origin)).unwrap_or_default(),
                            _ => String::new(),
                        },
                        Item::Retry => plx_platform::i18n::msg::livetv_setup_retry().to_owned(),
                        Item::Back => plx_platform::i18n::msg::livetv_setup_back().to_owned(),
                        Item::TurnOff => plx_platform::i18n::msg::livetv_setup_turn_off().to_owned(),
                        Item::Address => unreachable!(),
                    };
                    let fitted = measure.fit_line(&text, FIELD_W - 2.0 * FIELD_PAD, theme::size::BODY, true);
                    let w = (measure.width(&fitted, theme::size::BODY, true) + 44.0).min(FIELD_W);
                    TabPill::new(fitted.as_ptr(), theme::size::BODY, Rect::new(r.x, r.y, w, r.h))
                        .focused(on)
                        .draw(&plx_ui::Env::inert(), p);
                }
            }
        }
    }

    /// The address field: the typed text and a caret while the keyboard is up, the hint while it
    /// is empty.
    fn draw_field(&self, p: Painter, r: Rect, on: bool, tick_ms: u32, measure: &dyn plx_machine::machine::Measure) {
        let fill = if on { theme::CONTROL_IDLE_FILL } else { theme::CONTROL_IDLE_FILL_UNKEYED };
        p.rect(r, r.h * 0.5, fill, fill, 0.0);
        if on {
            p.rring(r, r.h * 0.5, 2.0, theme::CONTROL_RIM_FOCUS_UNKEYED);
        }
        let inner = Rect::new(r.x + FIELD_PAD, r.y, r.w - 2.0 * FIELD_PAD, r.h);
        let text = self.address.text();
        if text.is_empty() && !self.editing {
            line(p, measure, plx_platform::i18n::msg::livetv_setup_address(), theme::size::BODY, theme::CONTROL_IDLE_INK, true, inner, HAlign::Left);
            return;
        }
        if text.is_empty() {
            line(p, measure, plx_platform::i18n::msg::livetv_setup_address_hint(), theme::size::BODY, theme::TEXT_TERTIARY, false, inner, HAlign::Left);
        } else {
            line(p, measure, text, theme::size::BODY, theme::FIELD_EDITING_INK, false, inner, HAlign::Left);
        }
        if self.editing && (tick_ms / 530) % 2 == 0 {
            let before = &text[..self.address.caret().min(text.len())];
            let cx = inner.x + measure.width_str(before, theme::size::BODY, false);
            let cap = measure.cap_h(theme::size::BODY);
            p.rect(Rect::new(cx.min(inner.x + inner.w), r.y + (r.h - cap * 1.4) * 0.5, 2.0, cap * 1.4), 1.0,
                theme::FIELD_EDITING_INK, theme::FIELD_EDITING_INK, 0.0);
        }
    }
}
