//! **Playlist writes** — the four requests a virtual channel's playlist needs: create one from a
//! list of items, edit its title and description, replace its items, and delete it.
//!
//! These are writes to the profile's own playlists on its server, made only when the person keeps,
//! edits or deletes a channel. A virtual channel is an ordinary video playlist so it shows up — and
//! plays — in every Plex app; its recipe rides in the description (`plx_data::vchannel::recipe`).
//!
//! Shapes, from `docs/plex-openapi.json`: `POST /playlists?type=video&title=&smart=0&uri=` with a
//! `server://{machine}/com.plexapp.plugins.library/library/metadata/{rk,rk,...}` content URI;
//! `PUT /playlists/{id}` edits "in the same manner as editing metadata", whose fields are
//! `{field}.value` — sent beside the bare `title`/`summary` spelling older servers took, since a
//! server ignores the one it does not use; `PUT /playlists/{id}/items?uri=` adds items to a dumb
//! playlist (after `DELETE /playlists/{id}/items` clears it); `DELETE /playlists/{id}`.

use super::client::{Client, QueryBuilder};
use crate::http::Method;

/// Items a channel playlist carries at most: enough to play in another Plex app and to give the
/// playlist its poster, few enough that the content URI stays a modest request line.
pub const PLAYLIST_ITEMS_MAX: usize = 150;

/// A content URI naming items on `machine` by ratingKey.
pub fn items_uri(machine: &str, rating_keys: &[&str]) -> String {
    let keys: Vec<&str> = rating_keys.iter().copied().filter(|k| !k.is_empty()).take(PLAYLIST_ITEMS_MAX).collect();
    format!("server://{machine}/com.plexapp.plugins.library/library/metadata/{}", keys.join(","))
}

/// The create request, before the token — split out so its parameters are graded on the host.
fn create_query(machine: &str, title: &str, rating_keys: &[&str]) -> String {
    QueryBuilder::new("/playlists")
        .str("type", "video")
        .str("title", title)
        .int("smart", 0)
        .str("uri", &items_uri(machine, rating_keys))
        .build()
}

fn edit_query(rating_key: &str, title: &str, summary: &str) -> String {
    QueryBuilder::new(format!("/playlists/{rating_key}"))
        .str("title", title)
        .str("summary", summary)
        .str("title.value", title)
        .str("summary.value", summary)
        .int("title.locked", 1)
        .int("summary.locked", 1)
        .build()
}

impl Client {
    /// Create a video playlist of `rating_keys` titled `title`; its ratingKey, or `None` when the
    /// server refused or never answered.
    pub fn create_playlist(&self, title: &str, rating_keys: &[&str]) -> Option<String> {
        let page = self.post_json(&create_query(self.machine_id(), title, rating_keys))?;
        page.metadata.into_iter().next().map(|m| m.rating_key).filter(|rk| !rk.is_empty())
    }

    /// Set a playlist's title and description. `true` when the server accepted it.
    pub fn edit_playlist(&self, rating_key: &str, title: &str, summary: &str) -> bool {
        (200..300).contains(&self.put(&edit_query(rating_key, title, summary)))
    }

    /// Replace a dumb playlist's items with `rating_keys`.
    pub fn replace_playlist_items(&self, rating_key: &str, rating_keys: &[&str]) -> bool {
        let cleared = self
            .send_status(&format!("/playlists/{rating_key}/items"), Method::Delete)
            .is_some_and(|s| (200..300).contains(&s));
        if !cleared {
            return false;
        }
        let path = QueryBuilder::new(format!("/playlists/{rating_key}/items"))
            .str("uri", &items_uri(self.machine_id(), rating_keys))
            .build();
        (200..300).contains(&self.put(&path))
    }

    /// Delete a playlist. A 404 counts as done: it is already gone.
    pub fn delete_playlist(&self, rating_key: &str) -> bool {
        self.send_status(&format!("/playlists/{rating_key}"), Method::Delete)
            .is_some_and(|s| (200..300).contains(&s) || s == 404)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_create_request_names_its_items_on_the_server() {
        let q = create_query("abc123", "📺 90s Sitcoms", &["10", "", "11"]);
        assert!(q.starts_with("/playlists?type=video&title="), "{q}");
        assert!(q.contains("&smart=0&"), "{q}");
        assert!(q.contains(&format!("uri={}", super::super::urlenc_str("server://abc123/com.plexapp.plugins.library/library/metadata/10,11"))), "{q}");
    }

    #[test]
    fn an_edit_sends_both_spellings_and_locks_the_fields() {
        let q = edit_query("77", "T", "S");
        for part in ["title=T", "summary=S", "title.value=T", "summary.value=S", "title.locked=1", "summary.locked=1"] {
            assert!(q.contains(part), "{q} lacks {part}");
        }
    }

    #[test]
    fn a_long_item_list_is_capped() {
        let keys: Vec<String> = (0..400).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let uri = items_uri("m", &refs);
        assert_eq!(uri.rsplit('/').next().unwrap().split(',').count(), PLAYLIST_ITEMS_MAX);
    }
}
