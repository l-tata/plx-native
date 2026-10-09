//! **The account's Plex watchlist on Home** — the titles on the profile's watchlist (plex.tv's
//! Discover service, `plx_plex::plex::discover`) that the household's libraries hold, as those
//! LIBRARY rows, for Home's Watchlist shelf; and the list's membership, for the Add to / Remove
//! from Watchlist controls.
//!
//! A watchlist row is a CATALOG item (`plex://movie/<id>`), not a library one, so a row is shown
//! only once a server has been asked for that guid (`Client::find_by_guid`, the query "Also
//! available" uses) and answered with its own copy — the card then opens that copy's page exactly
//! as any shelf card does. A title no library holds is left off the shelf: the detail page reads a
//! server's metadata and has no catalog-only face. Its membership still counts, so its detail
//! page anywhere else would still read "Remove from Watchlist".
//!
//! **One fetch at a time, on a slow clock.** The list moves when somebody edits it, not on its own:
//! it is refetched with Home's hubs at most every [`STALE_MS`], and at once after an edit made here
//! (the worker performs the edit, then reads the list back, so the shelf always shows what plex.tv
//! says). The resolution costs one request per title per server, so it is bounded: the first
//! [`RESOLVE_MAX`] titles, stopping at a shelf's worth of copies.
//!
//! The state machine lives in `pms` (the hub store owns the shelf); this module is its pure halves
//! and the worker.

use std::collections::HashSet;
use std::sync::Arc;

use plx_plex::plex::discover::{watchlist_key, WatchlistItem};
use plx_plex::plex::ServerId;

use crate::pms::{PmsMovie, MAX_SHELF_ITEMS};

/// How long a fetched watchlist is trusted before Home's next hub refresh reads it again.
pub const STALE_MS: i64 = 10 * 60 * 1000;

/// Titles asked of the servers at most per fetch — the resolution's bound (module doc).
pub const RESOLVE_MAX: usize = 48;

/// The list's membership: every guid on it, as plex.tv listed them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Membership {
    guids: HashSet<String>,
}

impl Membership {
    pub fn new(guids: impl IntoIterator<Item = String>) -> Self {
        Self { guids: guids.into_iter().filter(|g| !watchlist_key(g).is_empty()).collect() }
    }

    /// Is the title with this guid on the list? `None` when it cannot be (a legacy or local guid,
    /// which has no catalog entry to add) — no control is offered for it.
    pub fn on_list(&self, guid: &str) -> Option<bool> {
        (!watchlist_key(guid).is_empty()).then(|| self.guids.contains(guid))
    }

    /// The membership after an edit made here, before plex.tv has been read back.
    pub fn edited(&self, guid: &str, add: bool) -> Self {
        let mut next = self.clone();
        if add {
            next.guids.insert(guid.to_owned());
        } else {
            next.guids.remove(guid);
        }
        next
    }
}

/// What one fetch produced: the membership and the shelf's library rows, in the list's order.
#[derive(Default, Clone)]
pub struct Build {
    pub members: Membership,
    pub rows: Vec<PmsMovie>,
}

/// **The shelf, from the list and a lookup.** For each title in the list's order (the most
/// recently added first), the first source — in roster order, our own servers first — whose
/// library holds a copy; at most a shelf's worth, asking about at most [`RESOLVE_MAX`] titles.
///
/// `find(sid, guid)` is one server's answer: `Some(Some(row))` a copy, `Some(None)` "not here",
/// `None` no answer (the next source is still asked). Pure, so the order and the bounds are graded
/// on the host; the worker hands in the real query.
pub fn resolve(
    items: &[WatchlistItem],
    sources: &[ServerId],
    mut find: impl FnMut(ServerId, &str) -> Option<Option<PmsMovie>>,
) -> Vec<PmsMovie> {
    let mut rows: Vec<PmsMovie> = Vec::new();
    let wanted = items.iter().filter(|it| matches!(it.kind.as_str(), "movie" | "show") && !watchlist_key(&it.guid).is_empty());
    for item in wanted.take(RESOLVE_MAX) {
        if rows.len() >= MAX_SHELF_ITEMS {
            break;
        }
        if let Some(row) = sources.iter().find_map(|&sid| find(sid, &item.guid).flatten()) {
            rows.push(row);
        }
    }
    rows
}

/// A server's copy among a `find_by_guid` answer's rows: the first one a shelf can draw (a title
/// and a poster, the rule every Home row meets), stamped with the guid it was asked for — a
/// server's own row can carry a legacy guid for the same title.
pub fn copy_of(mc: &plx_plex::plex::MediaContainer, sid: ServerId, guid: &str) -> Option<PmsMovie> {
    mc.metadata
        .iter()
        .filter(|it| crate::pms::listable(&it.kind))
        .map(|it| crate::pms::parse_item(it, sid))
        .find(|m| !m.title.is_empty() && !m.thumb.is_empty())
        .map(|mut m| {
            m.guid = guid.to_owned();
            m
        })
}

/// Everything one watchlist fetch needs, captured on the main thread at the spawn site.
pub struct Request {
    /// The hub store's watchlist generation when this was asked; a reset supersedes it.
    pub gen: u32,
    /// An edit to make first: `(guid, add)`.
    pub edit: Option<(String, bool)>,
    /// The sources to look titles up on, in roster order, each with the client captured for it.
    pub sources: Vec<(ServerId, &'static plx_plex::plex::Client)>,
    /// `(X-Plex-Client-Identifier, the profile's plex.tv token)`.
    pub credential: (String, String),
}

/// One finished fetch: `build` is `None` when the list could not be read.
pub struct Landing {
    pub gen: u32,
    pub build: Option<Build>,
}

/// The profile's plex.tv credential, when it has one this app may use for an account read
/// (`session::plex_tv_credential`); `None` signed out, or for a profile whose token is unknown.
///
/// A dev build whose plex.tv is the loopback stand-in (`account::plex_tv_is_stand_in`, which only a
/// `devtriggers` build can arm) reads the stand-in's synthetic list with a placeholder credential
/// when the profile has none — a simulator booted on an injected server token has no plex.tv
/// sign-in, and the stand-in accepts any token.
pub fn credential() -> Option<(String, String)> {
    let client_id = plx_plex::plex::session::peek().client_id.clone();
    let token = plx_plex::plex::session::current()
        .and_then(|user| plx_plex::plex::session::plex_tv_credential(&user))
        .or_else(|| plx_plex::plex::account::plex_tv_is_stand_in().then(|| "stand-in".to_owned()))?;
    Some((client_id, token))
}

/// WORKER THREAD: make the edit (if any), read the list, resolve the shelf.
#[cfg(not(any(test, feature = "test-support")))]
pub fn fetch(request: &Request) -> Option<Build> {
    let (client_id, token) = &request.credential;
    let account = plx_plex::plex::account::AccountClient::new(client_id, Some(token));
    if let Some((guid, add)) = &request.edit {
        if account.watchlist_edit(watchlist_key(guid), *add) != Some(true) {
            plx_base::eventlog::log(&format!("watchlist: the {} was not accepted", if *add { "add" } else { "removal" }));
        }
    }
    let items = account.watchlist()?;
    let sids: Vec<ServerId> = request.sources.iter().map(|(sid, _)| *sid).collect();
    let rows = resolve(&items, &sids, |sid, guid| {
        let client = request.sources.iter().find(|(s, _)| *s == sid)?.1;
        let mc = client.find_by_guid(guid)?;
        Some(copy_of(&mc, sid, guid))
    });
    plx_base::eventlog::log(&format!("watchlist: {} titles, {} on Home", items.len(), rows.len()));
    Some(Build { members: Membership::new(items.into_iter().map(|it| it.guid)), rows })
}

/// HOST SUITE: [`fetch`]'s cut — it reaches plex.tv and every server.
#[cfg(any(test, feature = "test-support"))]
pub fn fetch(_request: &Request) -> Option<Build> {
    None
}

/// The hub store's watchlist bookkeeping (main thread): the single flight, the clock, the
/// membership published to the controls, and an edit owed a read-back while a fetch is out.
#[derive(Default)]
pub struct State {
    pub gen: u32,
    pub fetching: bool,
    /// When the last fetch was ASKED (wall-clock ms); `None` before the first.
    pub asked_at_ms: Option<i64>,
    /// An edit made while a fetch was already out: it is performed by the next one.
    pub owed_edit: Option<(String, bool)>,
    /// `None` until a fetch has answered (the controls offer nothing while the list is unknown).
    pub members: Option<Arc<Membership>>,
}

impl State {
    /// Is a fetch due at `now_ms` from Home's refresh (not an edit)?
    pub fn due(&self, now_ms: i64) -> bool {
        !self.fetching && self.asked_at_ms.is_none_or(|at| now_ms - at >= STALE_MS || now_ms < at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, guid: &str) -> WatchlistItem {
        WatchlistItem { guid: guid.into(), kind: kind.into(), ..Default::default() }
    }

    fn copy(sid: ServerId, rk: &str) -> PmsMovie {
        PmsMovie { sid, rk: rk.into(), title: rk.into(), thumb: "/t".into(), ..Default::default() }
    }

    const OURS: ServerId = ServerId::from_raw(0);
    const SHARE: ServerId = ServerId::from_raw(1);

    #[test]
    fn each_title_takes_the_first_source_holding_it_in_list_order() {
        let items = [item("movie", "plex://movie/a"), item("show", "plex://show/b"), item("movie", "plex://movie/c")];
        let rows = resolve(&items, &[OURS, SHARE], |sid, guid| Some(match (sid, guid) {
            (OURS, "plex://movie/a") => Some(copy(OURS, "1")),
            (SHARE, "plex://movie/a") => Some(copy(SHARE, "9")),
            (SHARE, "plex://show/b") => Some(copy(SHARE, "2")),
            _ => None,
        }));
        let got: Vec<_> = rows.iter().map(|m| (m.sid, m.rk.as_str())).collect();
        assert_eq!(got, [(OURS, "1"), (SHARE, "2")], "our own copy first; a title nobody holds is left off");
    }

    #[test]
    fn a_source_that_does_not_answer_does_not_hide_a_copy_elsewhere() {
        let rows = resolve(&[item("movie", "plex://movie/a")], &[OURS, SHARE], |sid, _| match sid {
            OURS => None,
            _ => Some(Some(copy(SHARE, "5"))),
        });
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn episodes_and_unwatchlistable_guids_are_not_looked_up() {
        let mut asked = Vec::new();
        resolve(&[item("episode", "plex://episode/e"), item("movie", "com.plexapp.agents.imdb://tt1")], &[OURS], |_, guid| {
            asked.push(guid.to_owned());
            Some(None)
        });
        assert!(asked.is_empty());
    }

    #[test]
    fn the_resolution_is_bounded_by_the_shelf_and_by_the_titles_asked() {
        let many: Vec<_> = (0..200).map(|i| item("movie", &format!("plex://movie/{i}"))).collect();
        let mut asked = 0;
        let rows = resolve(&many, &[OURS], |_, _| { asked += 1; Some(Some(copy(OURS, "x"))) });
        assert_eq!((rows.len(), asked), (MAX_SHELF_ITEMS, MAX_SHELF_ITEMS), "a full shelf stops asking");
        let mut asked = 0;
        resolve(&many, &[OURS], |_, _| { asked += 1; Some(None) });
        assert_eq!(asked, RESOLVE_MAX, "nothing held: at most RESOLVE_MAX titles are asked about");
    }

    #[test]
    fn membership_answers_only_for_a_catalog_guid_and_follows_an_edit() {
        let m = Membership::new(["plex://movie/a".to_owned(), "local://3".to_owned()]);
        assert_eq!(m.on_list("plex://movie/a"), Some(true));
        assert_eq!(m.on_list("plex://movie/b"), Some(false));
        assert_eq!(m.on_list("local://3"), None, "no catalog entry: no control");
        assert_eq!(m.edited("plex://movie/b", true).on_list("plex://movie/b"), Some(true));
        assert_eq!(m.edited("plex://movie/a", false).on_list("plex://movie/a"), Some(false));
    }

    #[test]
    fn a_copy_is_a_drawable_row_stamped_with_the_asked_guid() {
        let mc: plx_plex::plex::MediaContainer = serde_json::from_str(
            r#"{"Metadata":[{"type":"movie","ratingKey":"1","title":"","thumb":"/t"},
                            {"type":"movie","ratingKey":"2","title":"s1","thumb":"/t","guid":"com.plexapp.agents.x://1"}]}"#,
        ).unwrap();
        let m = copy_of(&mc, OURS, "plex://movie/a").expect("the drawable row");
        assert_eq!((m.rk.as_str(), m.guid.as_str()), ("2", "plex://movie/a"));
    }

    #[test]
    fn a_fetch_is_due_once_the_list_is_stale_and_none_is_out() {
        let mut s = State::default();
        assert!(s.due(0), "never fetched");
        s.asked_at_ms = Some(1_000);
        assert!(!s.due(1_000 + STALE_MS - 1));
        assert!(s.due(1_000 + STALE_MS));
        s.fetching = true;
        assert!(!s.due(1_000 + STALE_MS), "one in flight");
    }
}
