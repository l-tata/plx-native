//! **The channel studio** — the Live TV page's face for the channels the app makes out of the
//! viewer's own library (`plx_data::vchannel`): the channels suggested for this profile, *Surprise
//! me*, the channels it kept, and the channel the viewer is making — from a show, season,
//! collection or playlist (the card menu's *Make a Channel*, [`Studio::open_make`]), or from the
//! library by its own options (*New Channel*, whose Genre and Decade options offer only values that
//! still air something, and which names itself after them, `recipe::name_for`). A channel being
//! made leads the suggestion row, is discarded rather than dismissed, and leaves the row once kept.
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
use plx_data::vchannel::catalog::Estimate;
use plx_data::vchannel::recipe::{Kinds, Recipe};
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
    /// *New Channel*: OK starts a channel built from the library by the viewer's own options.
    Create,
}

impl<'a> Card<'a> {
    /// The card's identity, stable across reloads: what the cursor and the drafts are keyed on.
    pub fn id(&self) -> String {
        match self {
            Card::Idea(s) => s.id.clone(),
            Card::Surprise(_) => SURPRISE_ID.to_owned(),
            Card::Channel(c) => format!("pl:{}", c.playlist),
            Card::Create => CREATE_ID.to_owned(),
        }
    }

    /// Is this the channel the viewer is making (from a title, or from the library)?
    pub fn is_made(&self) -> bool {
        matches!(self, Card::Idea(s) if s.id.starts_with(MADE_PREFIX))
    }

    /// The suggestion a draft is made from: the idea, or the surprise drawn.
    pub fn suggestion(&self) -> Option<&'a Suggestion> {
        match self {
            Card::Idea(s) => Some(s),
            Card::Surprise(s) => *s,
            Card::Channel(_) | Card::Create => None,
        }
    }
}

/// The New Channel card's identity.
pub const CREATE_ID: &str = "create";
/// The prefix of the id of the channel being made: it is shown as a suggestion card, but it is
/// the viewer's own and is discarded rather than dismissed.
pub const MADE_PREFIX: &str = "made:";

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
    /// Open the suggestion's options ([`EditItem`]).
    Edit,
    /// Drop the channel being made, without keeping it.
    Discard,
}

/// One of a suggestion's options, as the Edit row offers them left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditItem {
    /// A genre from the library's own (a channel built from the library only).
    Genre,
    /// A decade the library holds titles from (likewise).
    Decade,
    /// Films and shows, films only, shows only.
    Kinds,
    /// Only titles the profile has not watched.
    Unwatched,
    /// The highest content rating allowed.
    Rating,
    Done,
}

pub const EDIT_ITEMS: [EditItem; 4] = [EditItem::Kinds, EditItem::Unwatched, EditItem::Rating, EditItem::Done];
/// A channel built from the library chooses its genre and decade too.
pub const LIBRARY_EDIT_ITEMS: [EditItem; 6] =
    [EditItem::Genre, EditItem::Decade, EditItem::Kinds, EditItem::Unwatched, EditItem::Rating, EditItem::Done];

/// The options `recipe` offers: a channel the viewer is building from the library chooses what it
/// draws from; any other narrows what its source holds.
pub fn edit_items(recipe: &Recipe, made: bool) -> &'static [EditItem] {
    if made && recipe.source == plx_data::vchannel::recipe::Source::Library { &LIBRARY_EDIT_ITEMS } else { &EDIT_ITEMS }
}

/// The genres the Genre option cycles for `rules`: the library's, most titles first, keeping only
/// those that would still air something with the channel's other options as they are — so
/// cycling never lands on an empty channel.
pub fn genre_choices(cat: &plx_data::vchannel::catalog::Catalog, rules: &plx_data::vchannel::recipe::Rules) -> Vec<String> {
    library_genres(cat)
        .into_iter()
        .filter(|g| cat.estimate(&plx_data::vchannel::recipe::Rules { genres: vec![g.clone()], ..rules.clone() }).programmes > 0)
        .collect()
}

/// The decades the Decade option cycles for `rules`, oldest first, likewise only those that air
/// something with the other options.
pub fn decade_choices(cat: &plx_data::vchannel::catalog::Catalog, rules: &plx_data::vchannel::recipe::Rules) -> Vec<i64> {
    library_decades(cat)
        .into_iter()
        .filter(|d| cat.estimate(&plx_data::vchannel::recipe::Rules { year_from: *d, year_to: d + 9, ..rules.clone() }).programmes > 0)
        .collect()
}

/// The library's genres, most titles first, then by name.
pub fn library_genres(cat: &plx_data::vchannel::catalog::Catalog) -> Vec<String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    let mut spelled: HashMap<String, String> = HashMap::new();
    let genres = cat.movies.iter().flat_map(|p| p.genres.iter()).chain(cat.shows.iter().flat_map(|s| s.genres.iter()));
    for g in genres {
        let key = g.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        *count.entry(key.clone()).or_default() += 1;
        spelled.entry(key).or_insert_with(|| g.trim().to_owned());
    }
    let mut v: Vec<(String, usize)> = count.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().take(16).map(|(k, _)| spelled.remove(&k).unwrap_or(k)).collect()
}

/// The decades the library holds titles from, oldest first.
pub fn library_decades(cat: &plx_data::vchannel::catalog::Catalog) -> Vec<i64> {
    let mut v: Vec<i64> = cat.movies.iter().map(|p| p.year).chain(cat.shows.iter().map(|s| s.year))
        .filter(|y| *y >= 1900).map(|y| y / 10 * 10).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The value after `current` in `values`, wrapping through "any" (`None`).
fn cycle<T: Clone + PartialEq>(values: &[T], current: Option<&T>) -> Option<T> {
    match current.and_then(|c| values.iter().position(|v| v == c)) {
        Some(i) if i + 1 < values.len() => Some(values[i + 1].clone()),
        Some(_) => None,
        None => values.first().cloned(),
    }
}

/// What a timeline holds, for a channel whose source the catalog cannot count (a show, a
/// collection, a playlist): its films, its shows and its hours.
pub fn schedule_estimate(s: &Schedule) -> Estimate {
    let progs = s.programs();
    let mut shows: Vec<&str> = progs.iter().filter(|p| p.episode && !p.show_rk.is_empty()).map(|p| p.show_rk.as_str()).collect();
    shows.sort_unstable();
    shows.dedup();
    Estimate {
        films: progs.iter().filter(|p| !p.episode).count(),
        shows: shows.len(),
        programmes: progs.len(),
        hours: progs.iter().map(|p| p.dur_ms.max(0)).sum::<i64>() as f64 / 3_600_000.0,
    }
}

/// The next kinds, as the option cycles them.
pub fn next_kinds(k: Kinds) -> Kinds {
    match k {
        Kinds::Both => Kinds::Movies,
        Kinds::Movies => Kinds::Episodes,
        Kinds::Episodes => Kinds::Both,
    }
}

/// The next rating ceiling (a `recipe::rating_rank`; 0 = any): any, G, PG, PG-13, any.
pub fn next_rating(rank: u8) -> u8 {
    if rank >= 3 { 0 } else { rank + 1 }
}

/// A rating ceiling's name on the ladder `recipe::rating_rank` reads.
pub fn rating_name(rank: u8) -> &'static str {
    match rank {
        1 => "G",
        2 => "PG",
        3 => "PG-13",
        _ => "R",
    }
}

/// An option's label for `recipe`.
pub fn edit_label(item: EditItem, recipe: &Recipe) -> String {
    use plx_platform::i18n::msg;
    match item {
        EditItem::Kinds => match recipe.rules.kinds {
            Kinds::Both => msg::livetv_studio_kinds_both(),
            Kinds::Movies => msg::livetv_studio_kinds_movies(),
            Kinds::Episodes => msg::livetv_studio_kinds_episodes(),
        }
        .to_owned(),
        EditItem::Unwatched => msg::livetv_studio_unwatched().to_owned(),
        EditItem::Rating if recipe.rules.max_rating_rank == 0 => msg::livetv_studio_rating_any().to_owned(),
        EditItem::Rating => msg::livetv_studio_rating_up_to(rating_name(recipe.rules.max_rating_rank)),
        EditItem::Done => msg::livetv_studio_done().to_owned(),
        EditItem::Genre => recipe.rules.genres.first().cloned().unwrap_or_else(|| msg::livetv_studio_genre_any().to_owned()),
        EditItem::Decade if recipe.rules.year_from > 0 => plx_data::vchannel::suggest::decade_label(recipe.rules.year_from),
        EditItem::Decade => msg::livetv_studio_decade_any().to_owned(),
    }
}

/// What a channel holds, as the billboard reads it once its options changed: "12 films · 3 shows
/// · 40 hours", the empty parts left out.
pub fn estimate_line(e: &Estimate) -> String {
    use plx_platform::i18n::msg;
    let mut parts = Vec::new();
    if e.films > 0 {
        parts.push(msg::browse_person_films(e.films as i64));
    }
    if e.shows > 0 {
        parts.push(msg::browse_person_shows(e.shows as i64));
    }
    if e.hours >= 1.0 {
        parts.push(msg::livetv_studio_hours(e.hours.round() as i64));
    }
    parts.join(" \u{b7} ")
}

/// The actions of `card`, left to right.
pub fn acts(card: &Card<'_>) -> Vec<Act> {
    match card {
        c @ Card::Idea(_) if c.is_made() => vec![Act::Keep, Act::Reshuffle, Act::Order, Act::Edit, Act::Discard],
        Card::Idea(_) => vec![Act::Keep, Act::Reshuffle, Act::Order, Act::Edit, Act::NotInterested],
        Card::Create => Vec::new(),
        Card::Surprise(None) => vec![Act::Surprise],
        Card::Surprise(Some(_)) => vec![Act::Keep, Act::Surprise, Act::Reshuffle, Act::Order, Act::Edit],
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

/// The rows' cards for this view: the channel being made (`made`) first, then the suggestions and
/// *Surprise me* (offered once the library has been read); and the kept channels, then *New
/// Channel* (once the library has been read, since it is built from it).
pub fn rows<'a>(view: LiveTvView<'a>, made: Option<&'a Suggestion>) -> [Vec<Card<'a>>; 2] {
    let v = view.virtuals();
    let mut suggested: Vec<Card<'a>> = made.into_iter().map(Card::Idea).collect();
    suggested.extend(v.suggestions().iter().map(Card::Idea));
    if !v.suggestions().is_empty() || v.catalog().is_some() {
        suggested.push(Card::Surprise(v.surprise()));
    }
    let mut yours: Vec<Card<'a>> = v.channels().iter().map(Card::Channel).collect();
    if v.catalog().is_some() {
        yours.push(Card::Create);
    }
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

/// A new seed for Reshuffle: one under which something other than what `shown` airs at
/// `wall_ms` is on now, so a press always visibly changes the channel. Tries a few seeds in a
/// fixed sequence (deterministic for the press's instant); with one programme, or no timeline yet
/// to compare against, the first is as good as any.
pub fn fresh_seed(shown: Option<&Schedule>, seed: u64, wall_ms: i64) -> u64 {
    let candidate = |k: u64| mix(seed, (wall_ms as u64 | 1).wrapping_add(k));
    let Some(s) = shown.filter(|s| s.programs().len() > 1) else { return candidate(0) };
    let now = s.at(wall_ms).map(|slot| slot.program);
    (0..16).map(candidate).find(|&c| s.reseeded(c).at(wall_ms).map(|slot| slot.program) != now).unwrap_or_else(|| candidate(0))
}

/// Where the cursor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Cards,
    Actions,
    /// The suggestion's options ([`EDIT_ITEMS`]), in place of its actions.
    Edit,
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
    /// The option under the cursor while [`Zone::Edit`].
    pub edit: usize,
    /// What each edited draft holds, by suggestion id, worked out from the catalog when an
    /// option changed (so the billboard's count follows the options at once).
    estimates: HashMap<String, Estimate>,
    /// The channel being made, shown as the first card ("Make a Channel" on a title, or *New
    /// Channel*); its draft is in [`Self::drafts`] under its id.
    made: Option<Suggestion>,
    made_count: u64,
    /// The Keep in flight is of the channel being made (it leaves the row once kept).
    keeping_made: bool,
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
            edit: 0,
            estimates: HashMap::new(),
            made: None,
            made_count: 0,
            keeping_made: false,
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

    /// The channel being made, if any.
    pub fn made(&self) -> Option<&Suggestion> {
        self.made.as_ref()
    }

    /// Start making a channel from `recipe` ("Make a Channel" on a title): it becomes the first
    /// card, focused, and previews as soon as focus rests on it.
    pub fn open_make(&mut self, recipe: Recipe, why: String, view: LiveTvView<'_>) {
        self.made_count += 1;
        let id = format!("{MADE_PREFIX}{}", self.made_count);
        if let Some(old) = self.made.take() {
            self.drafts.remove(&old.id);
            self.estimates.remove(&old.id);
        }
        self.made = Some(Suggestion { id: id.clone(), name: recipe.name.clone(), why, tagline: String::new(), style: recipe.style, ..Default::default() });
        self.drafts.insert(id.clone(), Recipe { origin: String::new(), ..recipe });
        self.open = true;
        self.zone = Zone::Cards;
        self.want = Some(id);
        self.seen_done = view.virtuals().done().0;
    }

    /// *New Channel*: a channel built from the whole library, its options open at once.
    fn start_new(&mut self, view: LiveTvView<'_>, wall_ms: i64) {
        let rules = plx_data::vchannel::recipe::Rules::default();
        let name = plx_data::vchannel::recipe::name_for(&rules);
        let why = plx_platform::i18n::msg::livetv_studio_made_library().to_owned();
        let recipe = Recipe { rules, ..Recipe::made(plx_data::vchannel::recipe::Source::Library, &name, &why, wall_ms) };
        self.open_make(recipe, why, view);
        let id = self.made.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        if let (Some(cat), Some(r)) = (view.virtuals().catalog().map(|c| c.as_ref()), self.drafts.get(&id)) {
            self.estimates.insert(id.clone(), cat.estimate(&r.rules));
        }
        self.row = ROW_SUGGESTED;
        self.col[ROW_SUGGESTED] = 0;
        self.want = None;
        self.zone = Zone::Edit;
        self.edit = 0;
    }

    /// Forget the channel being made.
    fn drop_made(&mut self) {
        if let Some(m) = self.made.take() {
            self.drafts.remove(&m.id);
            self.estimates.remove(&m.id);
        }
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

    /// What an edited draft holds, when its options were changed.
    pub fn estimate_of(&self, s: &Suggestion) -> Option<&Estimate> {
        self.estimates.get(&s.id)
    }

    /// OK on an option: change the draft and preview it again, or close the options. A channel
    /// being built from the library renames itself after its options ("90s Sitcoms").
    fn apply_edit(&mut self, view: LiveTvView<'_>, wall_ms: i64, out: &mut Vec<Out>) {
        let made = self.made.clone();
        let rows = rows(view, made.as_ref());
        let Some(card) = self.focus(&rows) else { return };
        let is_made = card.is_made();
        let Some(s) = card.suggestion().cloned() else { return };
        let draft = self.draft(&s, wall_ms).clone();
        let items = edit_items(&draft, is_made);
        let item = items[self.edit.min(items.len() - 1)];
        let cat = view.virtuals().catalog().map(|c| c.as_ref());
        let mut next = draft.clone();
        match item {
            EditItem::Kinds => next.rules.kinds = next_kinds(next.rules.kinds),
            EditItem::Unwatched => next.rules.unwatched_only = !next.rules.unwatched_only,
            EditItem::Rating => next.rules.max_rating_rank = next_rating(next.rules.max_rating_rank),
            EditItem::Genre => {
                let genres = cat.map(|c| genre_choices(c, &next.rules)).unwrap_or_default();
                next.rules.genres = cycle(&genres, next.rules.genres.first()).into_iter().collect();
            }
            EditItem::Decade => {
                let decades = cat.map(|c| decade_choices(c, &next.rules)).unwrap_or_default();
                let now = (next.rules.year_from > 0).then_some(next.rules.year_from);
                match cycle(&decades, now.as_ref()) {
                    Some(d) => (next.rules.year_from, next.rules.year_to) = (d, d + 9),
                    None => (next.rules.year_from, next.rules.year_to) = (0, 0),
                }
            }
            EditItem::Done => {
                self.zone = Zone::Actions;
                self.act = acts(&card).iter().position(|a| *a == Act::Edit).unwrap_or(0);
                return;
            }
        }
        if is_made && next.source == plx_data::vchannel::recipe::Source::Library {
            next.name = plx_data::vchannel::recipe::name_for(&next.rules);
            if let Some(m) = self.made.as_mut() {
                m.name = next.name.clone();
            }
        }
        self.redraft(&s, wall_ms, move |r| *r = next, out);
        if let (Some(cat), Some(r)) = (cat, self.drafts.get(&s.id)) {
            if r.source == plx_data::vchannel::recipe::Source::Library {
                self.estimates.insert(s.id.clone(), cat.estimate(&r.rules));
            }
        }
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
                    if self.keeping_made {
                        self.drop_made();
                        self.keeping_made = false;
                    }
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
        let made = self.made.clone();
        let rows = rows(view, made.as_ref());
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
        let made = self.made.clone();
        let rows = rows(view, made.as_ref());
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
                self.keeping_made = s.id.starts_with(MADE_PREFIX);
                out.push(Out::Store(VCmd::Keep { token, recipe }));
            }
            (Act::Reshuffle, Card::Channel(c)) => {
                let mut recipe = c.recipe.clone();
                recipe.seed = fresh_seed(c.schedule.as_ref(), recipe.seed, wall_ms);
                out.push(Out::Store(VCmd::Update { playlist: c.playlist.clone(), recipe, rebuild: false }));
            }
            (Act::Order, Card::Channel(c)) => {
                let mut recipe = c.recipe.clone();
                recipe.style = next_style(recipe.style);
                out.push(Out::Store(VCmd::Update { playlist: c.playlist.clone(), recipe, rebuild: false }));
            }
            (Act::Reshuffle, card) => {
                let Some(s) = card.suggestion().cloned() else { return };
                let shown = self.schedule(&card, view);
                let seed = fresh_seed(shown, self.draft(&s, wall_ms).seed, wall_ms);
                self.redraft(&s, wall_ms, |r| r.seed = seed, out);
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
            (Act::Discard, _) => {
                self.drop_made();
                self.zone = Zone::Cards;
            }
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
            (Act::Edit, card) => {
                if let Some(s) = card.suggestion() {
                    let s = s.clone();
                    self.draft(&s, wall_ms);
                    self.zone = Zone::Edit;
                    self.edit = 0;
                }
            }
        }
    }

    /// A direction or OK / BACK. `false` hands the key to the engine (UP off the action row).
    pub fn key(&mut self, key: plx_machine::machine::Key, view: LiveTvView<'_>, wall_ms: i64, out: &mut Vec<Out>) -> bool {
        use plx_machine::machine::Key;
        let made = self.made.clone();
        let rows = rows(view, made.as_ref());
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
            (Zone::Edit, Key::Left) => self.edit = self.edit.saturating_sub(1),
            (Zone::Edit, Key::Right) => {
                let n = card.suggestion().and_then(|s| self.draft_of(s)).map_or(EDIT_ITEMS.len(), |r| edit_items(r, card.is_made()).len());
                self.edit = (self.edit + 1).min(n - 1);
            }
            (Zone::Edit, Key::Up) => return false,
            (Zone::Edit, Key::Down) => self.zone = Zone::Cards,
            (Zone::Edit, Key::Back) => self.zone = Zone::Actions,
            (Zone::Edit, Key::Ok) => self.apply_edit(view, wall_ms, out),
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
            (Zone::Cards, Key::Ok) if matches!(card, Card::Create) => self.start_new(view, wall_ms),
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
            "{}:{}:{}:{}:{:?}:{}:{}:{}:{}",
            self.open, self.row, self.col[0], self.col[1], self.zone, self.act,
            self.keeping.is_some(), self.confirm_delete.is_some(), self.edit
        )
    }
}

#[cfg(test)]
#[path = "studio_tests.rs"]
mod tests;
