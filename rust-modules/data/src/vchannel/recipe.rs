//! **A virtual channel's recipe**: everything a television needs to rebuild the channel — where
//! its programmes come from, the rules that narrow them, the order style, the seed and the start —
//! and the way it is kept on the viewer's Plex server so every television in the house shares it.
//!
//! The recipe rides in the description of an ordinary Plex playlist (one per channel, owned by the
//! profile, titled with [`TITLE_MARK`] so a person browsing playlists in another Plex app knows what
//! it is). The description starts with a sentence for that person and carries the recipe after
//! [`MARKER`] as one line of JSON; [`Recipe::from_summary`] finds it again and ignores everything
//! else, so a person who edits the visible text in Plex Web does not break the channel. Fields this
//! build does not know are dropped and missing ones default, so an older television reads a newer
//! recipe and the other way round.

use serde::{Deserialize, Serialize};

use super::schedule::Style;

/// What a channel's playlist description carries before the recipe JSON.
pub const MARKER: &str = "[plxnative-channel v1]";
/// The prefix of a channel playlist's title in other Plex apps.
pub const TITLE_MARK: &str = "📺 ";
/// The number the first virtual channel takes; later ones count up from the highest in use.
pub const FIRST_NUMBER: u32 = 900;

/// Where a channel's programmes come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A Plex playlist's items.
    Playlist { rk: String },
    /// A Plex collection's members (shows expand to their episodes).
    Collection { rk: String },
    /// A show's episodes, or a season's.
    Show { rk: String },
    Season { rk: String },
    /// The library, narrowed by [`Rules`] — a channel built from parameters or suggested.
    Library,
}

/// Movies, episodes, or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kinds {
    Movies,
    Episodes,
    #[default]
    Both,
}

impl Kinds {
    pub fn movies(self) -> bool {
        matches!(self, Kinds::Movies | Kinds::Both)
    }
    pub fn episodes(self) -> bool {
        matches!(self, Kinds::Episodes | Kinds::Both)
    }
}

/// The parameters a channel's programmes must meet. Every field is optional: an empty value means
/// "no constraint". With [`Source::Library`] they choose the programmes; with any other source they
/// only narrow it (a collection's films over 90 minutes, say).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub kinds: Kinds,
    /// Library sections to draw from; empty = every movie and TV library.
    pub sections: Vec<i64>,
    /// At least one of these genres (by name, any case).
    pub genres: Vec<String>,
    /// None of these genres.
    pub not_genres: Vec<String>,
    /// First and last year, inclusive; 0 = open.
    pub year_from: i64,
    pub year_to: i64,
    /// The highest content rating allowed, as a rank ([`rating_rank`]); 0 = any.
    pub max_rating_rank: u8,
    /// One of these studios or networks.
    pub studios: Vec<String>,
    /// One of these actors / directors.
    pub actors: Vec<String>,
    pub directors: Vec<String>,
    /// Only these shows (ratingKeys) — a "pick shows" channel.
    pub shows: Vec<String>,
    /// Programme length bounds in minutes; 0 = open.
    pub min_minutes: i64,
    pub max_minutes: i64,
    /// Minimum audience rating (0–10); 0 = any.
    pub min_score: f64,
    pub unwatched_only: bool,
    /// Never these shows or items (ratingKeys).
    pub exclude: Vec<String>,
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        *self == Rules::default()
    }
}

/// A channel's recipe. See the module doc for how it is stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub name: String,
    pub number: u32,
    pub source: Source,
    pub rules: Rules,
    #[serde(with = "style_key")]
    pub style: Style,
    pub seed: u64,
    pub epoch_ms: i64,
    /// The suggestion this channel was kept from (`suggest::Suggestion::id`), so the same idea is
    /// not suggested again; empty for a channel the person made.
    pub origin: String,
    /// A short line the guide shows under the channel's name ("Sitcoms from the 90s").
    pub tagline: String,
}

impl Default for Recipe {
    fn default() -> Self {
        Recipe {
            name: String::new(),
            number: FIRST_NUMBER,
            source: Source::Library,
            rules: Rules::default(),
            style: Style::Random,
            seed: 0,
            epoch_ms: 0,
            origin: String::new(),
            tagline: String::new(),
        }
    }
}

mod style_key {
    use super::Style;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(s: &Style, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(s.key())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Style, D::Error> {
        let key = String::deserialize(d).unwrap_or_default();
        Ok(Style::from_key(&key).unwrap_or_default())
    }
}

impl Recipe {
    /// The playlist description: a sentence for a person, then the marker and the recipe.
    pub fn to_summary(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        let blurb = if self.tagline.is_empty() {
            "A channel made by PlxNative.".to_owned()
        } else {
            format!("{} — a channel made by PlxNative.", self.tagline)
        };
        format!("{blurb}\n\n{MARKER}{json}")
    }

    /// The recipe in a playlist description, if it carries one.
    pub fn from_summary(summary: &str) -> Option<Recipe> {
        let at = summary.find(MARKER)?;
        let rest = &summary[at + MARKER.len()..];
        let line = rest.lines().next().unwrap_or("").trim();
        serde_json::from_str(line).ok()
    }

    /// The playlist's title in other Plex apps.
    pub fn playlist_title(&self) -> String {
        format!("{TITLE_MARK}{}", self.name)
    }
}

/// A channel name from a playlist title: the mark dropped.
pub fn name_of_title(title: &str) -> &str {
    title.strip_prefix(TITLE_MARK).unwrap_or(title).trim()
}

/// The number a new channel takes: one past the highest in use, starting at [`FIRST_NUMBER`].
pub fn next_number(in_use: impl IntoIterator<Item = u32>) -> u32 {
    in_use.into_iter().max().map_or(FIRST_NUMBER, |n| n.max(FIRST_NUMBER - 1) + 1)
}

/// A content rating's rank on one ladder across the US film and TV systems, so "PG and under"
/// admits TV-PG and TV-Y7 too. 0 for a rating this does not know (it is then never excluded).
pub fn rating_rank(rating: &str) -> u8 {
    let r = rating.trim().to_ascii_uppercase();
    let r = r.strip_prefix("US/").unwrap_or(&r);
    match r {
        "G" | "TV-Y" | "TV-G" | "TV-Y7" | "TV-Y7-FV" => 1,
        "PG" | "TV-PG" => 2,
        "PG-13" | "TV-14" => 3,
        "R" | "TV-MA" | "NC-17" | "NR" | "UNRATED" | "X" => 4,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Recipe {
        Recipe {
            name: "90s Sitcoms".into(),
            number: 903,
            source: Source::Library,
            rules: Rules { kinds: Kinds::Episodes, genres: vec!["Comedy".into()], year_from: 1990, year_to: 1999, ..Default::default() },
            style: Style::RoundRobin,
            seed: 0xdead_beef,
            epoch_ms: 1_700_000_000_000,
            origin: "decade:1990:comedy:episodes".into(),
            tagline: "Sitcoms from the 90s".into(),
        }
    }

    #[test]
    fn a_recipe_round_trips_through_a_playlist_description() {
        let r = sample();
        let summary = r.to_summary();
        assert!(summary.starts_with("Sitcoms from the 90s — a channel made by PlxNative."));
        assert_eq!(Recipe::from_summary(&summary), Some(r));
    }

    #[test]
    fn text_a_person_edits_around_the_marker_does_not_break_it_and_a_plain_playlist_is_none() {
        let r = sample();
        let edited = format!("My favourite!\n\n{}\n\nmore notes", r.to_summary());
        assert_eq!(Recipe::from_summary(&edited), Some(r));
        assert_eq!(Recipe::from_summary("Just a playlist"), None);
        assert_eq!(Recipe::from_summary(&format!("{MARKER}not json")), None);
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_default() {
        let json = r#"{"name":"X","style":"blocks","future":"field","source":{"kind":"show","rk":"42"}}"#;
        let r = Recipe::from_summary(&format!("{MARKER}{json}")).unwrap();
        assert_eq!((r.name.as_str(), r.style, r.number), ("X", Style::Blocks, FIRST_NUMBER));
        assert_eq!(r.source, Source::Show { rk: "42".into() });
        let odd = Recipe::from_summary(&format!(r#"{MARKER}{{"style":"sideways"}}"#)).unwrap();
        assert_eq!(odd.style, Style::Random, "an unknown style falls back rather than failing");
    }

    #[test]
    fn titles_and_numbers() {
        assert_eq!(sample().playlist_title(), "📺 90s Sitcoms");
        assert_eq!(name_of_title("📺 90s Sitcoms"), "90s Sitcoms");
        assert_eq!(name_of_title("Plain"), "Plain");
        assert_eq!(next_number([]), 900);
        assert_eq!(next_number([900, 903]), 904);
        assert_eq!(next_number([5]), 900, "a number below the range does not drag new ones down");
    }

    #[test]
    fn ratings_share_one_ladder() {
        assert_eq!(rating_rank("TV-PG"), rating_rank("PG"));
        assert!(rating_rank("TV-Y7") < rating_rank("PG-13"));
        assert_eq!(rating_rank("us/R"), 4);
        assert_eq!(rating_rank("Not a rating"), 0);
    }
}
