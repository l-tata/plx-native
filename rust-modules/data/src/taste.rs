//! **Recently added in your genres** — Home's shelf of new titles in the genres the profile
//! watches most, built per source on the hub worker (`pms::fetch_source`).
//!
//! *What you watch* is read from the server, as the profile itself: its Continue Watching deck,
//! plus each movie and TV library listed by `lastViewedAt` (newest first, the titles it has
//! actually played). Both are the PROFILE's own view state — PMS answers a listing's
//! `lastViewedAt` per user — where `/status/sessions/history/all` is the whole household's history
//! and needs the owner's token, which is why this does not read it. The genres of those titles are
//! counted ([`top_genres`]), and the top [`TOP_GENRES`] become the shelf: each genre's most
//! recently added titles in each library that has it, not yet watched, taking turns between the
//! genres so the favourite does not fill the row alone.
//!
//! It costs a handful of requests (the sections, one listing per library, one per genre per
//! library), so it runs on its own slower clock ([`STALE_MS`], `pms::Src::genres_at_ms`); a fetch
//! between keeps the rows the source last had.

use plx_plex::plex::{Client, MediaContainer, Metadata, ServerId};

use crate::pms::{parse_item, PmsMovie, MAX_SHELF_ITEMS};

/// How long a source's genre shelf is trusted before a hub refresh rebuilds it.
pub const STALE_MS: i64 = 30 * 60 * 1000;
/// How many genres the shelf draws from.
pub const TOP_GENRES: usize = 3;
/// Recently viewed titles read per library.
const VIEWED_PER_LIBRARY: i64 = 30;
/// Titles asked of each genre in each library.
const PER_GENRE: i64 = 12;
/// Libraries read at most (movie and TV ones only).
const LIBRARIES_MAX: usize = 4;

/// One genre as a library names it: the tag, and its id in that library (the `genre=` filter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genre {
    pub tag: String,
    /// `(library, tag id)` for each library the genre was seen in.
    pub ids: Vec<(i64, i64)>,
}

/// **The profile's top genres**, from the titles it watched: each title counts once per genre it
/// carries, a title in the deck counts double (it is what is being watched NOW), and ties keep
/// the order the genres were first seen in (most recent first). Genres are matched by name across
/// libraries; each keeps the id every library gave it. Pure.
pub fn top_genres(watched: &[(&Metadata, u32)], n: usize) -> Vec<Genre> {
    let mut seen: Vec<(Genre, u32)> = Vec::new();
    for (item, weight) in watched {
        for tag in item.genre.iter().filter(|t| !t.tag.is_empty()) {
            let at = (item.library_section_id, tag.id);
            match seen.iter_mut().find(|(g, _)| g.tag.eq_ignore_ascii_case(&tag.tag)) {
                Some((g, count)) => {
                    *count += weight;
                    if tag.id > 0 && !g.ids.contains(&at) {
                        g.ids.push(at);
                    }
                }
                None => seen.push((
                    Genre { tag: tag.tag.clone(), ids: if tag.id > 0 { vec![at] } else { Vec::new() } },
                    *weight,
                )),
            }
        }
    }
    // Stable: equal counts keep first-seen order.
    seen.sort_by(|a, b| b.1.cmp(&a.1));
    seen.into_iter().map(|(g, _)| g).filter(|g| !g.ids.is_empty()).take(n).collect()
}

/// The shelf from each genre's listings, in genre order: round-robin across the genres, a title
/// once, nothing already watched, at most a shelf's worth. Pure.
pub fn shelf(per_genre: Vec<Vec<PmsMovie>>) -> Vec<PmsMovie> {
    let depth = per_genre.iter().map(Vec::len).max().unwrap_or(0);
    let mut out: Vec<PmsMovie> = Vec::new();
    for i in 0..depth {
        for rows in &per_genre {
            let Some(m) = rows.get(i) else { continue };
            if m.watched || out.iter().any(|o| o.rk == m.rk) {
                continue;
            }
            out.push(m.clone());
            if out.len() == MAX_SHELF_ITEMS {
                return out;
            }
        }
    }
    out
}

/// WORKER THREAD: one source's genre shelf. `deck` is the source's Continue Watching answer,
/// already in hand. `None` when the server answered nothing usable at all (the fetch keeps the
/// rows it had); `Some(empty)` is a profile with no genres to go on.
pub fn fetch(c: &Client, sid: ServerId, deck: &MediaContainer) -> Option<Vec<PmsMovie>> {
    let sections = c.sections()?;
    let libraries: Vec<(i64, i64)> = sections
        .directory
        .iter()
        .filter_map(|d| {
            let kind = match d.kind.as_str() {
                "movie" => 1,
                "show" => 2,
                _ => return None,
            };
            Some((d.key.parse().ok()?, kind))
        })
        .take(LIBRARIES_MAX)
        .collect();
    let listing = |key: i64, kind: i64, sort: &str, filters: &[(String, String)], size: i64| {
        let mut all = vec![("type".to_owned(), kind.to_string())];
        all.extend_from_slice(filters);
        c.section_items_query(&plx_plex::plex::SectionQuery {
            section_key: key, sort, filters: &all, start: 0, size, include_meta: false,
        })
    };
    let viewed: Vec<MediaContainer> = libraries
        .iter()
        .filter_map(|&(key, kind)| listing(key, kind, "lastViewedAt:desc", &[], VIEWED_PER_LIBRARY))
        .collect();
    let mut watched: Vec<(&Metadata, u32)> = deck.hub.iter().flat_map(|h| h.metadata.iter()).map(|m| (m, 2)).collect();
    watched.extend(viewed.iter().flat_map(|mc| mc.metadata.iter()).filter(|m| m.last_viewed_at > 0).map(|m| (m, 1)));
    let genres = top_genres(&watched, TOP_GENRES);
    let per_genre: Vec<Vec<PmsMovie>> = genres
        .iter()
        .map(|g| {
            g.ids
                .iter()
                .filter_map(|&(lib, id)| {
                    let kind = libraries.iter().find(|(k, _)| *k == lib)?.1;
                    listing(lib, kind, "addedAt:desc", &[("genre".to_owned(), id.to_string())], PER_GENRE)
                })
                .flat_map(|mc| mc.metadata.into_iter())
                .map(|it| parse_item(&it, sid))
                .filter(|m| !m.title.is_empty() && !m.thumb.is_empty())
                .collect()
        })
        .collect();
    let rows = shelf(per_genre);
    plx_base::eventlog::log(&format!("hubs: source {} genres {} — {} titles", sid.raw(), genres.len(), rows.len()));
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(section: i64, genres: &[(i64, &str)]) -> Metadata {
        let tags: Vec<String> = genres.iter().map(|(id, t)| format!(r#"{{"id":{id},"tag":"{t}"}}"#)).collect();
        serde_json::from_str(&format!(r#"{{"librarySectionID":{section},"Genre":[{}]}}"#, tags.join(","))).unwrap()
    }

    fn tags(g: &[Genre]) -> Vec<&str> {
        g.iter().map(|g| g.tag.as_str()).collect()
    }

    #[test]
    fn the_most_watched_genres_lead_and_the_deck_counts_double() {
        let a = item(1, &[(10, "Drama"), (11, "Comedy")]);
        let b = item(1, &[(11, "Comedy")]);
        let c = item(2, &[(20, "Drama"), (21, "Horror")]);
        let d = item(1, &[(12, "Thriller")]);
        let e = item(1, &[(10, "Drama")]);
        let top = top_genres(&[(&d, 2), (&a, 1), (&b, 1), (&c, 1), (&e, 1)], 3);
        assert_eq!(tags(&top), ["Drama", "Thriller", "Comedy"],
            "Drama 3; Thriller 2 (one title in the deck) and Comedy 2: first seen breaks the tie");
        assert_eq!(top[0].ids, [(1, 10), (2, 20)], "one genre, each library's own id");
    }

    #[test]
    fn a_profile_with_nothing_watched_has_no_genres() {
        assert!(top_genres(&[], 3).is_empty());
        let untagged = item(1, &[]);
        assert!(top_genres(&[(&untagged, 1)], 3).is_empty());
    }

    #[test]
    fn the_shelf_takes_turns_between_genres_and_skips_the_watched() {
        let row = |rk: &str, watched: bool| PmsMovie { rk: rk.into(), watched, ..Default::default() };
        let rows = shelf(vec![
            vec![row("a1", false), row("a2", true), row("a3", false)],
            vec![row("b1", false), row("a1", false)],
        ]);
        let got: Vec<_> = rows.iter().map(|m| m.rk.as_str()).collect();
        assert_eq!(got, ["a1", "b1", "a3"]);
        let many = (0..100).map(|i| row(&i.to_string(), false)).collect();
        assert_eq!(shelf(vec![many]).len(), MAX_SHELF_ITEMS);
    }
}
