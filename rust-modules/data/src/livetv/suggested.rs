//! **Suggested Channels** — Home's shelf of the channels the library could air for this profile
//! (`crate::vchannel::suggest`), best first, and a *Surprise me* card at its end.
//!
//! A card is the suggestion's name over the poster of a title it would air, captioned with why it
//! is suggested ("Because you watch a lot of comedy"). Nothing here is a channel yet: OK opens the
//! suggestion in the channel studio on the Live TV page, where it is previewed and kept. The shelf
//! is a function of the suggestions alone, so it is rebuilt only when they move.

use crate::pms::{PmsMovie, KIND_CHANNEL_IDEA};
use crate::vchannel::suggest::Suggestion;

/// The id the *Surprise me* card carries in `rk`; never a suggestion's id (those name a family).
pub const SURPRISE_ID: &str = "surprise";

/// Cards the shelf holds at most, the surprise card included.
pub const SUGGESTED_MAX: usize = 13;

/// The shelf's rows for `suggestions` (best first): one card each, then *Surprise me* — which is
/// offered only beside real suggestions, since it draws from the same library they did.
pub fn rows(suggestions: &[Suggestion], surprise_title: &str, surprise_why: &str) -> Vec<PmsMovie> {
    if suggestions.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<PmsMovie> = suggestions.iter().take(SUGGESTED_MAX - 1).map(card).collect();
    out.push(PmsMovie {
        kind: KIND_CHANNEL_IDEA,
        rk: SURPRISE_ID.to_owned(),
        title: surprise_title.to_owned(),
        show_title: surprise_why.to_owned(),
        ..Default::default()
    });
    out
}

fn card(s: &Suggestion) -> PmsMovie {
    let (sid, thumb) = s.posters.first().cloned().unwrap_or_default();
    let (art_sid, art) = s.art.clone().unwrap_or_default();
    PmsMovie {
        sid: plx_plex::plex::ServerId::from_raw(if thumb.is_empty() { art_sid } else { sid }),
        kind: KIND_CHANNEL_IDEA,
        rk: s.id.clone(),
        title: s.name.clone(),
        show_title: s.why.clone(),
        thumb,
        art,
        child_count: s.programmes as _,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idea(id: &str, poster: Option<&str>) -> Suggestion {
        Suggestion {
            id: id.into(),
            name: format!("Name {id}"),
            why: format!("Why {id}"),
            posters: poster.map(|p| vec![(3, p.to_owned())]).unwrap_or_default(),
            art: Some((3, format!("/art/{id}"))),
            programmes: 40,
            ..Default::default()
        }
    }

    #[test]
    fn one_card_per_suggestion_then_surprise() {
        let rows = rows(&[idea("a", Some("/p/a")), idea("b", None)], "Surprise Me", "Something new");
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.kind == KIND_CHANNEL_IDEA));
        assert_eq!((rows[0].rk.as_str(), rows[0].title.as_str(), rows[0].show_title.as_str()), ("a", "Name a", "Why a"));
        assert_eq!((rows[0].sid.raw(), rows[0].thumb.as_str()), (3, "/p/a"));
        assert_eq!(rows[1].thumb, "", "no poster: the tile names the channel");
        assert_eq!((rows[2].rk.as_str(), rows[2].title.as_str()), (SURPRISE_ID, "Surprise Me"));
    }

    #[test]
    fn no_suggestions_no_shelf_and_the_shelf_is_capped() {
        assert!(rows(&[], "S", "W").is_empty());
        let many: Vec<Suggestion> = (0..30).map(|i| idea(&i.to_string(), None)).collect();
        let r = rows(&many, "S", "W");
        assert_eq!(r.len(), SUGGESTED_MAX);
        assert_eq!(r.last().unwrap().rk, SURPRISE_ID);
    }
}
