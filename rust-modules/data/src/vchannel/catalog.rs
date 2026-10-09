//! **The library, as virtual channels see it**: every film and show of the viewer's movie and TV
//! libraries with the facts rules and suggestions read (genres, year, network or studio, rating,
//! people, collections, labels, how much is watched), and each show's episodes on demand.
//!
//! [`Catalog::fetch`] reads it with the profile's own token on a worker — one paged listing per
//! library — and [`Catalog::programmes`] narrows it by a recipe's [`Rules`]. A rule channel's
//! episodes are read per matching show ([`episodes`]); the store caches them.
//!
//! Pure apart from the two fetches.

use plx_plex::plex::{Client, Metadata};

use super::recipe::{rating_rank, Rules};
use super::schedule::Program;

/// Rows asked per page of a library listing.
const PAGE: i64 = 500;
/// Rows read per library at most.
const PER_SECTION_MAX: i64 = 6_000;

/// One show, with the facts a rule or a suggestion reads at show level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Show {
    pub rk: String,
    pub sid: u16,
    pub section: i64,
    pub title: String,
    pub year: i64,
    pub genres: Vec<String>,
    /// The network (PMS's `studio` on a show).
    pub studio: String,
    pub content_rating: String,
    pub audience_rating: f64,
    pub summary: String,
    pub thumb: String,
    pub art: String,
    pub actors: Vec<String>,
    pub collections: Vec<String>,
    pub labels: Vec<String>,
    /// Episodes, episodes watched, and a typical episode's length.
    pub leaf_count: i64,
    pub viewed_leaf_count: i64,
    pub episode_ms: i64,
    pub added_at: i64,
    pub last_viewed_at: i64,
}

impl Show {
    pub fn unwatched(&self) -> i64 {
        (self.leaf_count - self.viewed_leaf_count).max(0)
    }

    /// A show's runtime, estimated from its episode count and typical length.
    pub fn estimated_ms(&self) -> i64 {
        self.leaf_count.max(0) * self.episode_ms.max(0)
    }
}

/// One movie or TV library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    pub key: i64,
    pub movies: bool,
    pub title: String,
}

/// Every film and show of the profile's movie and TV libraries on one server.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub sid: u16,
    pub sections: Vec<Section>,
    pub movies: Vec<Program>,
    pub shows: Vec<Show>,
}

fn tags(v: &[plx_plex::plex::Tag]) -> Vec<String> {
    v.iter().map(|t| t.tag.clone()).filter(|t| !t.is_empty()).collect()
}

/// A film or an episode as a programme. An episode carries its show's genres, network and rating
/// when its own row lacks them (`show`).
pub fn programme_of(m: &Metadata, sid: u16, show: Option<&Show>) -> Program {
    let episode = m.kind == "episode";
    let mut p = Program {
        rk: m.rating_key.clone(),
        sid,
        episode,
        title: m.title.clone(),
        show_title: if episode { m.grandparent_title.clone() } else { String::new() },
        show_rk: if episode { m.grandparent_rating_key.clone() } else { String::new() },
        season: if episode { m.parent_index } else { 0 },
        index: if episode { m.index } else { 0 },
        dur_ms: m.duration,
        year: m.year,
        genres: tags(&m.genre),
        summary: m.summary.clone(),
        content_rating: m.content_rating.clone(),
        studio: m.studio.clone(),
        thumb: m.thumb.clone(),
        art: m.art.clone(),
        audience_rating: m.audience_rating,
        watched: m.view_count > 0,
        section: m.library_section_id,
        directors: tags(&m.director),
        actors: tags(&m.role),
        collections: tags(&m.collection),
        labels: tags(&m.label),
        added_at: m.added_at,
        last_viewed_at: m.last_viewed_at,
    };
    if let Some(s) = show {
        if p.genres.is_empty() {
            p.genres = s.genres.clone();
        }
        if p.studio.is_empty() {
            p.studio = s.studio.clone();
        }
        if p.content_rating.is_empty() {
            p.content_rating = s.content_rating.clone();
        }
        if p.year == 0 {
            p.year = s.year;
        }
        if p.art.is_empty() {
            p.art = s.art.clone();
        }
        if p.section == 0 {
            p.section = s.section;
        }
        if p.show_title.is_empty() {
            p.show_title = s.title.clone();
        }
        if p.show_rk.is_empty() {
            p.show_rk = s.rk.clone();
        }
    }
    p
}

pub fn show_of(m: &Metadata, sid: u16) -> Show {
    Show {
        rk: m.rating_key.clone(),
        sid,
        section: m.library_section_id,
        title: m.title.clone(),
        year: m.year,
        genres: tags(&m.genre),
        studio: m.studio.clone(),
        content_rating: m.content_rating.clone(),
        audience_rating: m.audience_rating,
        summary: m.summary.clone(),
        thumb: m.thumb.clone(),
        art: m.art.clone(),
        actors: tags(&m.role),
        collections: tags(&m.collection),
        labels: tags(&m.label),
        leaf_count: m.leaf_count,
        viewed_leaf_count: m.viewed_leaf_count,
        episode_ms: m.duration,
        added_at: m.added_at,
        last_viewed_at: m.last_viewed_at,
    }
}

/// A show's episodes (`allLeaves`), in natural order.
pub fn episodes(c: &Client, show: &Show) -> Option<Vec<Program>> {
    let page = c.all_leaves(&show.rk)?;
    let sid = c.id().raw();
    Some(page.metadata.iter().filter(|m| m.kind == "episode").map(|m| programme_of(m, sid, Some(show))).collect())
}

fn any_eq(have: &[String], want: &[String]) -> bool {
    want.iter().any(|w| have.iter().any(|h| h.eq_ignore_ascii_case(w)))
}

fn year_ok(r: &Rules, year: i64) -> bool {
    (r.year_from == 0 || year >= r.year_from) && (r.year_to == 0 || (year > 0 && year <= r.year_to))
}

fn rating_ok(r: &Rules, rating: &str) -> bool {
    r.max_rating_rank == 0 || {
        let rank = rating_rank(rating);
        rank != 0 && rank <= r.max_rating_rank
    }
}

fn section_ok(r: &Rules, section: i64) -> bool {
    r.sections.is_empty() || r.sections.contains(&section)
}

/// Does a film meet the rules?
pub fn film_matches(r: &Rules, p: &Program) -> bool {
    r.kinds.movies()
        && r.shows.is_empty()
        && section_ok(r, p.section)
        && (r.genres.is_empty() || any_eq(&p.genres, &r.genres))
        && !any_eq(&p.genres, &r.not_genres)
        && year_ok(r, p.year)
        && rating_ok(r, &p.content_rating)
        && (r.studios.is_empty() || any_eq(&[p.studio.clone()], &r.studios))
        && (r.actors.is_empty() || any_eq(&p.actors, &r.actors))
        && (r.directors.is_empty() || any_eq(&p.directors, &r.directors))
        && length_ok(r, p.dur_ms)
        && (r.min_score <= 0.0 || p.audience_rating >= r.min_score)
        && (!r.unwatched_only || !p.watched)
        && !r.exclude.contains(&p.rk)
}

/// Does a show meet the rules at show level (its episodes are then checked one by one)?
pub fn show_matches(r: &Rules, s: &Show) -> bool {
    r.kinds.episodes()
        && (r.shows.is_empty() || r.shows.contains(&s.rk))
        && section_ok(r, s.section)
        && (r.genres.is_empty() || any_eq(&s.genres, &r.genres))
        && !any_eq(&s.genres, &r.not_genres)
        && year_ok(r, s.year)
        && rating_ok(r, &s.content_rating)
        && (r.studios.is_empty() || any_eq(&[s.studio.clone()], &r.studios))
        && (r.actors.is_empty() || any_eq(&s.actors, &r.actors))
        && r.directors.is_empty()
        && (r.min_score <= 0.0 || s.audience_rating >= r.min_score)
        && (!r.unwatched_only || s.unwatched() > 0)
        && !r.exclude.contains(&s.rk)
}

fn length_ok(r: &Rules, dur_ms: i64) -> bool {
    let mins = dur_ms / 60_000;
    (r.min_minutes == 0 || mins >= r.min_minutes) && (r.max_minutes == 0 || mins <= r.max_minutes)
}

/// Does an episode of a matching show meet the episode-level rules?
pub fn episode_matches(r: &Rules, p: &Program) -> bool {
    length_ok(r, p.dur_ms) && (!r.unwatched_only || !p.watched) && !r.exclude.contains(&p.rk)
}

/// The programme-level rules any source honours (a collection's films over 90 minutes, say).
pub fn narrows(r: &Rules, p: &Program) -> bool {
    length_ok(r, p.dur_ms)
        && (!r.unwatched_only || !p.watched)
        && !r.exclude.contains(&p.rk)
        && !(p.episode && r.exclude.contains(&p.show_rk))
        && rating_ok(r, &p.content_rating)
}

/// What a rule set would hold, estimated from the catalog alone (no episode reads): films and
/// shows that match, and the hours they add up to. The builder's live count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    pub films: usize,
    pub shows: usize,
    pub programmes: usize,
    pub hours: f64,
}

impl Catalog {
    /// Read every movie and TV library on `c`'s server.
    pub fn fetch(c: &Client) -> Option<Catalog> {
        let sid = c.id().raw();
        let page = c.sections()?;
        let mut cat = Catalog { sid, ..Default::default() };
        for d in &page.directory {
            let Ok(key) = d.key.parse::<i64>() else { continue };
            let movies = match d.kind.as_str() {
                "movie" => true,
                "show" => false,
                _ => continue,
            };
            cat.sections.push(Section { key, movies, title: d.title.clone() });
            let mut start = 0;
            while start < PER_SECTION_MAX {
                let Some(rows) = c.section_items_paged(key, start, PAGE) else { break };
                let n = rows.metadata.len() as i64;
                for m in &rows.metadata {
                    let mut m_section = m.library_section_id;
                    if m_section == 0 {
                        m_section = key;
                    }
                    match (movies, m.kind.as_str()) {
                        (true, "movie") => {
                            let mut p = programme_of(m, sid, None);
                            p.section = m_section;
                            cat.movies.push(p);
                        }
                        (false, "show") => {
                            let mut s = show_of(m, sid);
                            s.section = m_section;
                            cat.shows.push(s);
                        }
                        _ => {}
                    }
                }
                start += n;
                if n < PAGE || (rows.total_size > 0 && start >= rows.total_size) {
                    break;
                }
            }
        }
        Some(cat)
    }

    /// The films that meet `r`.
    pub fn films<'a>(&'a self, r: &'a Rules) -> impl Iterator<Item = &'a Program> + 'a {
        self.movies.iter().filter(move |p| film_matches(r, p))
    }

    /// The shows whose episodes may meet `r`.
    pub fn shows_matching<'a>(&'a self, r: &'a Rules) -> impl Iterator<Item = &'a Show> + 'a {
        self.shows.iter().filter(move |s| show_matches(r, s))
    }

    pub fn show(&self, rk: &str) -> Option<&Show> {
        self.shows.iter().find(|s| s.rk == rk)
    }

    /// The builder's live count for `r`.
    pub fn estimate(&self, r: &Rules) -> Estimate {
        let mut e = Estimate::default();
        let mut ms: i64 = 0;
        for p in self.films(r) {
            e.films += 1;
            ms += p.dur_ms;
        }
        for s in self.shows_matching(r) {
            e.shows += 1;
            let eps = if r.unwatched_only { s.unwatched() } else { s.leaf_count.max(0) };
            e.programmes += eps as usize;
            ms += eps * s.episode_ms.max(0);
        }
        e.programmes += e.films;
        e.hours = ms as f64 / 3_600_000.0;
        e
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
