//! **The channel studio** — the Live TV page's face for the channels the app makes out of the
//! viewer's own library (`plx_data::vchannel`): the channels suggested for this profile, *Surprise
//! me*, and the channels it kept.
//!
//! The page is a billboard over two rows. The billboard is the focused card, full bleed: the
//! picture of what would be on right now, the channel's name, why it is suggested, what it holds,
//! and its live preview — the programme on now with how far in it is, and the next few — so a
//! suggestion is judged by what it would actually air, not by its name. Under the copy sit its
//! actions. Under the billboard, *Suggested for You* (best first, *Surprise me* last) and *Your
//! Channels*.
//!
//! **Focus** stays inside the Live TV page's one engine element, as the guide's does: the studio
//! keeps a cursor that is either on a card or on one of the focused card's actions. OK on a card
//! moves to its actions; BACK from the actions returns to the card, and from a card leaves the
//! studio (to the guide it was opened from, or off the page). UP from the actions is the engine's,
//! to the top strip.
//!
//! **A preview is asked for once focus rests.** Resting on a suggestion for [`PREVIEW_DWELL_MS`]
//! asks the store to build its timeline (`VCmd::Preview`, one at a time, tagged with a token so a
//! stale answer is never shown). A kept channel already has its timeline. Reshuffle and Order
//! change the draft — the seed, the order style — and ask again; on a kept channel they write the
//! change to its playlist (`VCmd::Update`), so every television in the house follows.
//!
//! **Keep** writes the draft as a channel (`VCmd::Keep`); the store reports the outcome
//! (`Channels::done`), and the studio confirms it and moves focus to the new channel. Delete asks
//! for a second press. Nothing here writes view state: a channel only reads the library.

use std::collections::HashMap;

use plx_data::livetv::suggested::SURPRISE_ID;
use plx_data::livetv::LiveTvView;
use plx_data::vchannel::channels::{VChannel, VCmd, WriteDone};
use plx_data::vchannel::recipe::Recipe;
use plx_data::vchannel::schedule::{mix, Program, Schedule, Style};
use plx_data::vchannel::suggest::Suggestion;

/// How long focus rests on a suggestion before its timeline is built.
pub const PREVIEW_DWELL_MS: u32 = 300;
/// How long a confirmation or an error stays on screen.
pub const TOAST_MS: u32 = 3_500;
/// How many programmes the preview lists after the one on now.
pub const UP_NEXT: usize = 3;

/// The two rows.
pub const ROW_SUGGESTED: usize = 0;
pub const ROW_YOURS: usize = 1;

/// One card.
#[derive(Clone, Copy, Debug)]
pub enum Card<'a> {
    Idea(&'a Suggestion),
    /// *Surprise me*, with the channel it last drew.
    Surprise(Option<&'a Suggestion>),
    Channel(&'a VChannel),
}

impl<'a> Card<'a> {
    /// The card's identity, stable across reloads: what the cursor and the drafts are keyed on.
    pub fn id(&self) -> String {
        match self {
            Card::Idea(s) => s.id.clone(),
            Card::Surprise(_) => SURPRISE_ID.to_owned(),
            Card::Channel(c) => format!("pl:{}", c.playlist),
        }
    }

    /// The suggestion a draft is made from: the idea, or the surprise drawn.
    pub fn suggestion(&self) -> Option<&'a Suggestion> {
        match self {
            Card::Idea(s) => Some(s),
            Card::Surprise(s) => *s,
            Card::Channel(_) => None,
        }
    }
}

/// What a card's action row offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Keep,
    Reshuffle,
    Order,
    NotInterested,
    Surprise,
    Watch,
    Delete,
}

/// The actions of `card`, left to right.
pub fn acts(card: &Card<'_>) -> Vec<Act> {
    match card {
        Card::Idea(_) => vec![Act::Keep, Act::Reshuffle, Act::Order, Act::NotInterested],
        Card::Surprise(None) => vec![Act::Surprise],
        Card::Surprise(Some(_)) => vec![Act::Keep, Act::Surprise, Act::Reshuffle, Act::Order],
        Card::Channel(_) => vec![Act::Watch, Act::Reshuffle, Act::Order, Act::Delete],
    }
}

/// The next order style, as the Order button cycles them.
pub fn next_style(s: Style) -> Style {
    let i = Style::ALL.iter().position(|x| *x == s).unwrap_or(0);
    Style::ALL[(i + 1) % Style::ALL.len()]
}

/// A style's name, as the Order button reads it.
pub fn style_name(s: Style) -> &'static str {
    use plx_platform::i18n::msg;
    match s {
        Style::Random => msg::livetv_studio_style_random(),
        Style::RoundRobin => msg::livetv_studio_style_rotation(),
        Style::Blocks => msg::livetv_studio_style_blocks(),
        Style::InOrder => msg::livetv_studio_style_order(),
    }
}

/// The rows' cards for this view: the suggestions then *Surprise me* (offered once the library has
/// been read), and the kept channels.
pub fn rows(view: LiveTvView<'_>) -> [Vec<Card<'_>>; 2] {
    let v = view.virtuals();
    let mut suggested: Vec<Card<'_>> = v.suggestions().iter().map(Card::Idea).collect();
    if !suggested.is_empty() || v.catalog().is_some() {
        suggested.push(Card::Surprise(v.surprise()));
    }
    let yours = v.channels().iter().map(Card::Channel).collect();
    [suggested, yours]
}

/// One programme as the preview lists it: the show and the episode for an episode
/// ("Frasier · S3 · E4 · The Focus Group"), the title and year for a film.
pub fn programme_line(p: &Program) -> String {
    if p.episode && !p.show_title.is_empty() {
        let code = p.episode_code();
        return [p.show_title.as_str(), code.as_str(), p.title.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{b7} ");
    }
    if p.year > 0 { format!("{} ({})", p.title, p.year) } else { p.title.clone() }
}

/// Where the cursor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Cards,
    Actions,
}

/// What a key or a press asks the page to do beyond the studio's own state.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Store(VCmd),
    /// Tune the lineup channel with this number.
    Tune(String),
    /// Leave the studio (to the guide, or off the page when the studio is the page).
    Close,
}

/// The studio's state. See the module doc.
#[derive(Debug)]
pub struct Studio {
    /// Opened from the guide (BACK returns there). The studio is also the page by itself when
    /// there is no Tunarr server and no kept channel; then this stays false.
    pub open: bool,
    pub row: usize,
    pub col: [usize; 2],
    pub zone: Zone,
    pub act: usize,
    /// A card to put the cursor on once it exists (a Home card opened the studio, a channel was
    /// just kept).
    pub want: Option<String>,
    /// The drafts made of suggestions, by card id ([`Card::id`]; the surprise's by its drawn id).
    drafts: HashMap<String, Recipe>,
    /// The token of the last preview asked for, and the draft identity it was for.
    token: u64,
    previewed: Option<(String, u64)>,
    /// Focus rests on this card since this tick.
    rest: Option<(String, u32)>,
    /// A Keep is in flight, with its token.
    pub keeping: Option<u64>,
    /// Delete was pressed once on this channel.
    pub confirm_delete: Option<String>,
    /// How many store writes this page has read (`Channels::done`).
    seen_done: u64,
    /// A confirmation or an error, and when it appeared.
    pub toast: Option<(String, u32)>,
}

impl Default for Studio {
    fn default() -> Self {
        Studio {
            open: false,
            row: ROW_SUGGESTED,
            col: [0, 0],
            zone: Zone::Cards,
            act: 0,
            want: None,
            drafts: HashMap::new(),
            token: 0,
            previewed: None,
            rest: None,
            keeping: None,
            confirm_delete: None,
            seen_done: 0,
            toast: None,
        }
    }
}

impl Studio {
    /// Open from the guide (or a Home card), on the card `want` when given.
    pub fn open_on(&mut self, want: Option<String>, view: LiveTvView<'_>) {
        self.open = true;
        self.zone = Zone::Cards;
        self.want = want.filter(|w| !w.is_empty());
        self.seen_done = view.virtuals().done().0;
    }

    /// The focused card, after clamping the cursor to the rows.
    pub fn focus<'a>(&self, rows: &'a [Vec<Card<'a>>; 2]) -> Option<Card<'a>> {
        rows[self.row].get(self.col[self.row]).copied()
    }

    /// Keep the cursor on cards that exist, and seat a wanted card once it does.
    pub fn clamp(&mut self, rows: &[Vec<Card<'_>>; 2]) {
        if let Some(want) = self.want.clone() {
            for (r, row) in rows.iter().enumerate() {
                if let Some(c) = row.iter().position(|c| c.id() == want) {
                    self.row = r;
                    self.col[r] = c;
                    self.want = None;
                    break;
                }
            }
        }
        for (r, row) in rows.iter().enumerate() {
            self.col[r] = self.col[r].min(row.len().saturating_sub(1));
        }
        if rows[self.row].is_empty() {
            let other = 1 - self.row;
            if !rows[other].is_empty() {
                self.row = other;
            }
        }
        if let Some(card) = self.focus(rows) {
            self.act = self.act.min(acts(&card).len().saturating_sub(1));
        } else {
            self.zone = Zone::Cards;
        }
    }

    /// The draft for a suggestion: made once, on first sight, and kept through reshuffles.
    pub fn draft(&mut self, s: &Suggestion, now_ms: i64) -> &Recipe {
        self.drafts.entry(s.id.clone()).or_insert_with(|| s.recipe(now_ms))
    }

    pub fn draft_of(&self, s: &Suggestion) -> Option<&Recipe> {
        self.drafts.get(&s.id)
    }

    /// The timeline the billboard previews for `card`: a kept channel's own, or the store's
    /// preview when it is the answer to this draft's last request.
    pub fn schedule<'a>(&self, card: &Card<'a>, view: LiveTvView<'a>) -> Option<&'a Schedule> {
        match card {
            Card::Channel(c) => c.schedule.as_ref(),
            _ => {
                let s = card.suggestion()?;
                let (id, token) = self.previewed.as_ref()?;
                let p = view.virtuals().preview()?;
                (*id == s.id && p.token == *token).then_some(())?;
                p.schedule.as_ref()
            }
        }
    }

    /// Did this draft's preview come back empty (nothing in the library fits)?
    pub fn unbuildable(&self, card: &Card<'_>, view: LiveTvView<'_>) -> bool {
        let Some(s) = card.suggestion() else { return false };
        let Some((id, token)) = self.previewed.as_ref() else { return false };
        view.virtuals().preview().is_some_and(|p| *id == s.id && p.token == *token && p.failed)
    }

    /// One tick: read the store's write outcomes, expire the toast, and ask for a preview once
    /// focus has rested on a suggestion whose draft has none.
    pub fn tick(&mut self, view: LiveTvView<'_>, now_ms: u32, wall_ms: i64, focused: bool, out: &mut Vec<Out>) -> bool {
        let mut changed = false;
        let v = view.virtuals();
        for done in v.done_since(self.seen_done) {
            match done {
                WriteDone::Kept { token, playlist } if self.keeping == Some(*token) => {
                    self.keeping = None;
                    let number = v.channel(playlist).map(|c| c.number()).unwrap_or_default();
                    self.toast = Some((plx_platform::i18n::msg::livetv_studio_kept(&number), now_ms));
                    self.want = Some(format!("pl:{playlist}"));
                    self.zone = Zone::Cards;
                    changed = true;
                }
                WriteDone::Failed { token } if self.keeping == Some(*token) || *token == 0 => {
                    self.keeping = None;
                    self.toast = Some((plx_platform::i18n::msg::livetv_studio_failed().to_owned(), now_ms));
                    changed = true;
                }
                _ => {}
            }
        }
        self.seen_done = v.done().0;
        if self.toast.as_ref().is_some_and(|(_, at)| now_ms.wrapping_sub(*at) > TOAST_MS) {
            self.toast = None;
            changed = true;
        }
        let rows = rows(view);
        self.clamp(&rows);
        let Some(card) = self.focus(&rows).filter(|_| focused) else {
            self.rest = None;
            return changed;
        };
        let Some(s) = card.suggestion() else {
            self.rest = None;
            return changed;
        };
        if self.previewed.as_ref().is_some_and(|(id, _)| *id == s.id) {
            return changed;
        }
        match &self.rest {
            Some((id, since)) if *id == s.id => {
                if now_ms.wrapping_sub(*since) >= PREVIEW_DWELL_MS {
                    let s = s.clone();
                    self.ask_preview(&s, wall_ms, out);
                    changed = true;
                }
            }
            _ => self.rest = Some((s.id.clone(), now_ms)),
        }
        changed
    }

    fn ask_preview(&mut self, s: &Suggestion, wall_ms: i64, out: &mut Vec<Out>) {
        self.token += 1;
        let recipe = self.draft(s, wall_ms).clone();
        self.previewed = Some((s.id.clone(), self.token));
        out.push(Out::Store(VCmd::Preview { token: self.token, recipe }));
    }

    /// The draft of `s` changed: preview it again.
    fn redraft(&mut self, s: &Suggestion, wall_ms: i64, change: impl FnOnce(&mut Recipe), out: &mut Vec<Out>) {
        let mut recipe = self.draft(s, wall_ms).clone();
        change(&mut recipe);
        self.drafts.insert(s.id.clone(), recipe);
        self.ask_preview(s, wall_ms, out);
    }

    /// Perform the focused card's action.
    pub fn activate(&mut self, view: LiveTvView<'_>, wall_ms: i64, out: &mut Vec<Out>) {
        let rows = rows(view);
        let Some(card) = self.focus(&rows) else { return };
        let list = acts(&card);
        let Some(act) = list.get(self.act.min(list.len().saturating_sub(1))).copied() else { return };
        if act != Act::Delete {
            self.confirm_delete = None;
        }
        match (act, card) {
            (Act::Keep, card) => {
                let Some(s) = card.suggestion() else { return };
                if self.keeping.is_some() {
                    return;
                }
                let s = s.clone();
                // The preview's own token, when it is this draft's: the store keeps the channel
                // from the programmes it already read.
                let token = match &self.previewed {
                    Some((id, token)) if *id == s.id => *token,
                    _ => {
                        self.token += 1;
                        self.token
                    }
                };
                let recipe = self.draft(&s, wall_ms).clone();
                self.keeping = Some(token);
                out.push(Out::Store(VCmd::Keep { token, recipe }));
            }
            (Act::Reshuffle, Card::Channel(c)) => {
                let mut recipe = c.recipe.clone();
                recipe.seed = mix(recipe.seed, wall_ms as u64 | 1);
                out.push(Out::Store(VCmd::Update { playlist: c.playlist.clone(), recipe, rebuild: false }));
            }
            (Act::Order, Card::Channel(c)) => {
                let mut recipe = c.recipe.clone();
                recipe.style = next_style(recipe.style);
                out.push(Out::Store(VCmd::Update { playlist: c.playlist.clone(), recipe, rebuild: false }));
            }
            (Act::Reshuffle, card) => {
                let Some(s) = card.suggestion().cloned() else { return };
                self.redraft(&s, wall_ms, |r| r.seed = mix(r.seed, wall_ms as u64 | 1), out);
            }
            (Act::Order, card) => {
                let Some(s) = card.suggestion().cloned() else { return };
                self.redraft(&s, wall_ms, |r| r.style = next_style(r.style), out);
            }
            (Act::NotInterested, Card::Idea(s)) => {
                self.drafts.remove(&s.id);
                out.push(Out::Store(VCmd::Dismiss { id: s.id.clone() }));
                self.zone = Zone::Cards;
            }
            (Act::NotInterested, _) => {}
            (Act::Surprise, _) => {
                out.push(Out::Store(VCmd::Surprise { seed: mix(wall_ms as u64, self.token + 1) }));
                self.token += 1;
                self.act = 0;
            }
            (Act::Watch, Card::Channel(c)) => out.push(Out::Tune(c.number())),
            (Act::Watch, _) => {}
            (Act::Delete, Card::Channel(c)) => {
                if self.confirm_delete.as_deref() == Some(c.playlist.as_str()) {
                    self.confirm_delete = None;
                    self.zone = Zone::Cards;
                    out.push(Out::Store(VCmd::Delete { playlist: c.playlist.clone() }));
                } else {
                    self.confirm_delete = Some(c.playlist.clone());
                }
            }
            (Act::Delete, _) => {}
        }
    }

    /// A direction or OK / BACK. `false` hands the key to the engine (UP off the action row).
    pub fn key(&mut self, key: plx_machine::machine::Key, view: LiveTvView<'_>, wall_ms: i64, out: &mut Vec<Out>) -> bool {
        use plx_machine::machine::Key;
        let rows = rows(view);
        self.clamp(&rows);
        let Some(card) = self.focus(&rows) else {
            return match key {
                Key::Back => {
                    out.push(Out::Close);
                    true
                }
                Key::Up => false,
                _ => true,
            };
        };
        let n_acts = acts(&card).len();
        match (self.zone, key) {
            (Zone::Actions, Key::Left) => self.act = self.act.saturating_sub(1),
            (Zone::Actions, Key::Right) => self.act = (self.act + 1).min(n_acts.saturating_sub(1)),
            (Zone::Actions, Key::Up) => return false,
            (Zone::Actions, Key::Down) | (Zone::Actions, Key::Back) => {
                self.zone = Zone::Cards;
                self.confirm_delete = None;
            }
            (Zone::Actions, Key::Ok) => self.activate(view, wall_ms, out),
            (Zone::Cards, Key::Left) => self.col[self.row] = self.col[self.row].saturating_sub(1),
            (Zone::Cards, Key::Right) => {
                self.col[self.row] = (self.col[self.row] + 1).min(rows[self.row].len().saturating_sub(1));
            }
            (Zone::Cards, Key::Up) => {
                if self.row == ROW_YOURS && !rows[ROW_SUGGESTED].is_empty() {
                    self.row = ROW_SUGGESTED;
                } else {
                    self.zone = Zone::Actions;
                    self.act = 0;
                }
            }
            (Zone::Cards, Key::Down) => {
                if self.row == ROW_SUGGESTED && !rows[ROW_YOURS].is_empty() {
                    self.row = ROW_YOURS;
                }
            }
            (Zone::Cards, Key::Ok) => {
                self.zone = Zone::Actions;
                self.act = 0;
            }
            (Zone::Cards, Key::Back) => out.push(Out::Close),
            (_, Key::Other) => return false,
        }
        let moved_to = self.focus(&rows).map(|c| c.id());
        if moved_to != Some(card.id()) {
            self.act = 0;
            self.confirm_delete = None;
        }
        true
    }

    pub fn canon(&self) -> String {
        format!(
            "{}:{}:{}:{}:{:?}:{}:{}:{}",
            self.open, self.row, self.col[0], self.col[1], self.zone, self.act,
            self.keeping.is_some(), self.confirm_delete.is_some()
        )
    }
}

#[cfg(test)]
#[path = "studio_tests.rs"]
mod tests;
