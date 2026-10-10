//! **The server side of virtual channels**: the blocking reads and writes the store's workers run
//! with the profile's own client — list the profile's channel playlists, read a source, keep a
//! channel (create its playlist), update one (its title and description, and its items), delete
//! one.

use plx_plex::plex::collections::CollectionOutcome;
use plx_plex::plex::Client;

use super::build::Reads;
use super::catalog::{episodes, programme_of, show_of, Catalog, Show};
use super::recipe::{name_of_title, Recipe, Source, TITLE_MARK};
use super::schedule::Program;

/// Members read per page of a playlist or collection.
const PAGE: i64 = 300;
/// Members read from one playlist or collection at most.
const MEMBERS_MAX: i64 = 4_000;

/// A channel the server holds: its playlist and its recipe (the recipe's name follows the
/// playlist's title, so a rename in another Plex app shows here).
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub playlist: String,
    pub recipe: Recipe,
    /// The playlist's composite poster, for the guide's channel tile.
    pub composite: String,
}

/// The profile's channels on `c`'s server, by number.
pub fn list(c: &Client) -> Option<Vec<Stored>> {
    let page = c.video_playlists()?;
    let mut out = Vec::new();
    for m in page.metadata.iter().filter(|m| m.title.starts_with(TITLE_MARK)) {
        // A listing row may omit the description; read the playlist itself then.
        let summary = if m.summary.is_empty() {
            c.metadata(&m.rating_key).map(|full| full.summary).unwrap_or_default()
        } else {
            m.summary.clone()
        };
        let Some(mut recipe) = Recipe::from_summary(&summary) else { continue };
        recipe.name = name_of_title(&m.title).to_owned();
        out.push(Stored { playlist: m.rating_key.clone(), recipe, composite: m.composite.clone() });
    }
    out.sort_by_key(|s| s.recipe.number);
    Some(out)
}

/// Keep a channel: create its playlist with its first items. The playlist's ratingKey.
pub fn keep(c: &Client, recipe: &Recipe, programmes: &[Program]) -> Option<String> {
    let keys: Vec<&str> = programmes.iter().map(|p| p.rk.as_str()).collect();
    let rk = c.create_playlist(&recipe.playlist_title(), &keys)?;
    if !c.edit_playlist(&rk, &recipe.playlist_title(), &recipe.to_summary()) {
        // A playlist with no recipe would be a stray: take it back rather than leave it.
        let _ = c.delete_playlist(&rk);
        return None;
    }
    Some(rk)
}

/// Write a changed recipe (a rename, a reshuffle, new rules) to its playlist; with `programmes`,
/// refresh the items too.
pub fn update(c: &Client, playlist: &str, recipe: &Recipe, programmes: Option<&[Program]>) -> bool {
    let ok = c.edit_playlist(playlist, &recipe.playlist_title(), &recipe.to_summary());
    if let Some(p) = programmes.filter(|p| !p.is_empty()) {
        let keys: Vec<&str> = p.iter().map(|x| x.rk.as_str()).collect();
        let _ = c.replace_playlist_items(playlist, &keys);
    }
    ok
}

pub fn delete(c: &Client, playlist: &str) -> bool {
    c.delete_playlist(playlist)
}

fn paged(mut read: impl FnMut(i64) -> CollectionOutcome) -> Option<Vec<plx_plex::plex::Metadata>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < MEMBERS_MAX {
        let page = match read(start) {
            CollectionOutcome::Ok(page) => page,
            _ if start > 0 => break,
            _ => return None,
        };
        let n = page.metadata.len() as i64;
        let total = page.total_size;
        out.extend(page.metadata);
        start += n;
        if n < PAGE || (total > 0 && start >= total) {
            break;
        }
    }
    Some(out)
}

/// [`Reads`] over a live client, with the catalog for show facts and a cache for episodes.
pub struct ClientReads<'a> {
    pub client: &'a Client,
    pub catalog: Option<&'a Catalog>,
    pub cache: &'a std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Vec<Program>>>>,
}

impl ClientReads<'_> {
    fn cached_episodes(&mut self, show: &Show) -> Option<Vec<Program>> {
        if let Some(hit) = self.cache.lock().ok().and_then(|m| m.get(&show.rk).cloned()) {
            return Some(hit.as_ref().clone());
        }
        let list = episodes(self.client, show)?;
        if let Ok(mut m) = self.cache.lock() {
            m.insert(show.rk.clone(), std::sync::Arc::new(list.clone()));
        }
        Some(list)
    }

    /// The catalog's facts for a show, or a minimal entry when the catalog does not know it.
    fn show_entry(&self, rk: &str) -> Show {
        self.catalog.and_then(|c| c.show(rk)).cloned().unwrap_or_else(|| {
            self.client.metadata(rk).map(|m| show_of(&m, self.client.id().raw())).unwrap_or(Show { rk: rk.to_owned(), ..Default::default() })
        })
    }

    /// Films stay; a show expands to its episodes; a season to its own.
    fn expand(&mut self, rows: Vec<plx_plex::plex::Metadata>) -> Vec<Program> {
        let sid = self.client.id().raw();
        let mut out = Vec::new();
        for m in rows {
            match m.kind.as_str() {
                "movie" => out.push(programme_of(&m, sid, None)),
                "episode" => {
                    let show = self.catalog.and_then(|c| c.show(&m.grandparent_rating_key)).cloned();
                    out.push(programme_of(&m, sid, show.as_ref()));
                }
                "show" => {
                    let s = self.show_entry(&m.rating_key);
                    if let Some(list) = self.cached_episodes(&s) {
                        out.extend(list);
                    }
                }
                "season" => {
                    let s = self.show_entry(&m.parent_rating_key);
                    if let Some(page) = self.client.children(&m.rating_key) {
                        out.extend(page.metadata.iter().filter(|e| e.kind == "episode").map(|e| programme_of(e, sid, Some(&s))));
                    }
                }
                _ => {}
            }
        }
        out
    }
}

impl Reads for ClientReads<'_> {
    fn source(&mut self, source: &Source) -> Option<Vec<Program>> {
        let c = self.client;
        let rows = match source {
            Source::Playlist { rk } => paged(|start| c.playlist_items(rk, start, PAGE))?,
            Source::Collection { rk } => paged(|start| c.collection_children(rk, start, PAGE))?,
            Source::Show { rk } => {
                let s = self.show_entry(rk);
                return self.cached_episodes(&s);
            }
            Source::Season { rk } => c.children(rk)?.metadata.into_iter().map(|mut m| {
                if m.kind.is_empty() {
                    m.kind = "episode".into();
                }
                m
            }).collect(),
            Source::Library => return None,
        };
        Some(self.expand(rows))
    }

    fn episodes(&mut self, show: &Show) -> Option<Vec<Program>> {
        self.cached_episodes(show)
    }
}
