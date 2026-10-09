//! **What the guide shows, and what it says about time** — pure rules over the lineup, host-tested
//! exactly as the page uses them.
//!
//! * [`Filter`] and [`chips`] — the genre strip above the grid: *All*, the four families every
//!   guide is scanned for (films, sport, kids, news), the two other genres the grid colours when the
//!   guide has any, then the guide's own most frequent categories that name no genre, capped at
//!   [`MAX_CHIPS`].
//! * [`visible`] — the channel rows a filter keeps: the channels with at least one matching airing
//!   in the visible window. **Non-matching airings on a kept row are DIMMED, not hidden** (the page
//!   draws them at [`DIM`]): a row with holes punched in it reads as a guide with missing data, and
//!   the airing before a match is exactly the context a viewer needs to tune in time for it. A
//!   whole channel with nothing matching IS hidden, because a dimmed row is just a taller hole.
//! * [`Timing`] — "12 min left" for an airing on now, "Starts in 25 min" for one coming up within
//!   the hour; beyond that the start time the pane already prints is the clearer fact.
//! * [`genre_edge`] — the accent a programme cell's edge wears.

use std::borrow::Cow;
use std::collections::HashMap;

use plx_data::livetv::guide::{Airing, Genre, Lineup};
use plx_ui::theme;

/// The most chips the strip offers, *All* included — a strip the width of the grid at the shipped
/// type sizes.
pub const MAX_CHIPS: usize = 9;
/// The genres every strip offers, present or not: the families a viewer opens a guide to find.
pub const FIXED: [Genre; 4] = [Genre::Movie, Genre::Sports, Genre::Kids, Genre::News];
/// The opacity a non-matching airing is drawn at while a filter is on.
pub const DIM: f32 = 0.32;
/// An airing this close is announced as "Starts in N min"; a later one by its start time alone.
pub const SOON_MS: i64 = 60 * 60 * 1000;

/// One chip of the genre strip: what the grid is narrowed to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    #[default]
    All,
    Genre(Genre),
    /// A category as the guide spells it, matched ignoring case.
    Category(String),
}

impl Filter {
    /// Does `a` belong to this filter? A genre reads EVERY category (a guide that files a cartoon
    /// as `Comedy`, `Animation` belongs under Kids and Comedy both), unlike the cell's colour, which
    /// is its first genre.
    pub fn matches(&self, a: &Airing) -> bool {
        match self {
            Filter::All => true,
            Filter::Genre(g) => a.categories.iter().any(|c| Genre::of_category(c) == Some(*g)),
            Filter::Category(c) => a.has_category(c),
        }
    }

    /// The filter's canonical spelling, for the page's logical state.
    pub fn canon(&self) -> String {
        match self {
            Filter::All => "all".into(),
            Filter::Genre(g) => format!("genre:{g:?}"),
            Filter::Category(c) => format!("category:{}", c.to_lowercase()),
        }
    }

    /// What the chip says: the app's own word for a genre, the guide's own for a category.
    pub fn label(&self) -> Cow<'static, str> {
        use plx_platform::i18n::msg;
        match self {
            Filter::All => Cow::Borrowed(msg::livetv_filter_all()),
            Filter::Genre(g) => Cow::Borrowed(genre_label(*g)),
            Filter::Category(c) => Cow::Owned(c.clone()),
        }
    }
}

/// The app's name for a genre.
pub fn genre_label(g: Genre) -> &'static str {
    use plx_platform::i18n::msg;
    match g {
        Genre::Movie => msg::livetv_filter_movies(),
        Genre::Sports => msg::livetv_filter_sports(),
        Genre::Kids => msg::livetv_filter_kids(),
        Genre::News => msg::livetv_filter_news(),
        Genre::Documentary => msg::livetv_filter_documentary(),
        Genre::Comedy => msg::livetv_filter_comedy(),
    }
}

/// **The strip's chips for this lineup**, in order: *All*, [`FIXED`], Documentary and Comedy when
/// any airing is one, then the categories no genre claims, most frequent first (ties by name, so
/// the strip does not reshuffle between two loads of one guide), up to [`MAX_CHIPS`] in all.
pub fn chips(lineup: &Lineup) -> Vec<Filter> {
    let mut out = vec![Filter::All];
    out.extend(FIXED.iter().map(|g| Filter::Genre(*g)));
    let mut present = [false; Genre::ALL.len()];
    // Counted by lower-cased spelling; the first spelling met is the one the chip prints.
    let mut counts: HashMap<String, (usize, String)> = HashMap::new();
    for ch in &lineup.channels {
        for a in &ch.airings {
            for c in &a.categories {
                match Genre::of_category(c) {
                    Some(g) => present[Genre::ALL.iter().position(|x| *x == g).unwrap_or(0)] = true,
                    None if !c.is_empty() => {
                        counts.entry(c.to_lowercase()).or_insert_with(|| (0, c.clone())).0 += 1;
                    }
                    None => {}
                }
            }
        }
    }
    for (i, g) in Genre::ALL.iter().enumerate() {
        if present[i] && !FIXED.contains(g) {
            out.push(Filter::Genre(*g));
        }
    }
    let mut rest: Vec<(usize, String)> = counts.into_values().collect();
    rest.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    for (_, spelling) in rest {
        if out.len() >= MAX_CHIPS {
            break;
        }
        out.push(Filter::Category(spelling));
    }
    out.truncate(MAX_CHIPS);
    out
}

/// **The lineup rows `filter` keeps** over the window `[from, to)`: every row under *All*; under
/// any other filter, the channels with an airing in the window that matches.
pub fn visible(lineup: &Lineup, filter: &Filter, from: i64, to: i64) -> Vec<usize> {
    if *filter == Filter::All {
        return (0..lineup.len()).collect();
    }
    lineup
        .channels
        .iter()
        .enumerate()
        .filter(|(_, ch)| ch.airings.iter().any(|a| a.stop_ms > from && a.start_ms < to && filter.matches(a)))
        .map(|(i, _)| i)
        .collect()
}

/// Where `prev` (a lineup index) lands in a new row list: its own row when it is still shown, else
/// the first row after it in lineup order, else the last row. `0` for an empty list.
pub fn reseat(rows: &[usize], prev: Option<usize>) -> usize {
    let Some(prev) = prev else { return 0 };
    rows.iter().position(|&r| r >= prev).unwrap_or(rows.len().saturating_sub(1))
}

/// What the guide says about an airing's place in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    /// On now, this many ms to go.
    Left(i64),
    /// Coming up within [`SOON_MS`], starting in this many ms.
    StartsIn(i64),
    /// Over, or further ahead than [`SOON_MS`].
    Quiet,
}

impl Timing {
    pub fn of(a: &Airing, now: i64) -> Timing {
        if a.covers(now) {
            Timing::Left(a.stop_ms - now)
        } else if a.start_ms > now && a.start_ms - now <= SOON_MS {
            Timing::StartsIn(a.start_ms - now)
        } else {
            Timing::Quiet
        }
    }

    /// The words: `12 min left` (the shared Continue Watching formatter, rounded up to the minute),
    /// `Starts in 25 min`.
    pub fn text(self) -> Option<String> {
        match self {
            Timing::Left(ms) => Some(plx_ui::fmt::time_left(ms)),
            Timing::StartsIn(ms) => {
                // Rounded UP to the minute and floored at one: "in 0 min" says nothing.
                let ms = ((ms + 59_999) / 60_000).max(1) * 60_000;
                Some(plx_platform::i18n::msg::livetv_starts_in(&plx_ui::fmt::dur_long(ms)))
            }
            Timing::Quiet => None,
        }
    }
}

/// The accent a programme cell's leading edge wears: its genre's token, `None` for an airing whose
/// categories name no genre.
pub fn genre_edge(a: &Airing) -> Option<[f32; 4]> {
    Some(match a.genre()? {
        Genre::Movie => theme::GUIDE_GENRE_MOVIE,
        Genre::Sports => theme::GUIDE_GENRE_SPORTS,
        Genre::Kids => theme::GUIDE_GENRE_KIDS,
        Genre::News => theme::GUIDE_GENRE_NEWS,
        Genre::Documentary => theme::GUIDE_GENRE_DOCUMENTARY,
        Genre::Comedy => theme::GUIDE_GENRE_COMEDY,
    })
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
