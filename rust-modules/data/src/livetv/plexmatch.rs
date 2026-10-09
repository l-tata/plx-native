//! **Is this airing in the viewer's Plex library?** A film or an episode on a Tunarr channel is very
//! often a file on one of the viewer's own servers (Tunarr builds its channels FROM Plex), and then
//! the guide can offer it from the start instead of joined in the middle.
//!
//! Three layers, the first two pure:
//!
//! * [`Want`] — what an airing could be found as: a film (title and, when the guide gives one,
//!   year) or an episode (show, `SxxEyy`, episode title). An airing that is neither (no title) asks
//!   nothing.
//! * [`best`] — the MATCH RULES over the rows a search returned ([`Candidate`]). They are strict on
//!   purpose: offering the wrong film is worse than offering none, so a match needs the normalised
//!   title to be equal, never "contains"; a film needs its year to agree when both sides know one
//!   and, without a year, all same-titled films to agree on theirs; an episode needs its show and
//!   either its season and episode numbers or (when the guide has none) its own title.
//! * [`lookup`] — the blocking half the store runs on a worker: `/hubs/search` on every registered
//!   server ([`plx_plex::plex::Client::search`], as the Search screen fans out), and for an episode
//!   whose numbers are known but whose title the search did not find, the show's episode list
//!   (`allLeaves`).
//!
//! The store caches one answer per [`Want::key`], scoped to the server roster and the profile the
//! answer was found under ([`Scope`]): a profile switch can take a library away.

use super::guide::{Airing, Genre};
use plx_plex::plex::{Metadata, ServerId};

/// What an airing can be looked up as.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Want {
    Film { title: String, year: Option<u16> },
    Episode { show: String, season: Option<u32>, episode: Option<u32>, title: String },
}

impl Want {
    /// What `a` could be in a Plex library. An airing with episode facts (`SxxEyy`, an episode
    /// title) is an episode of the show its title names; anything else with a title is looked up
    /// as a film — the match rules then refuse whatever is not one.
    pub fn of(a: &Airing) -> Option<Want> {
        if normalise(&a.title).is_empty() {
            return None;
        }
        let (season, episode) = parse_episode(&a.episode);
        let is_film = a.genre() == Some(Genre::Movie);
        if !is_film && (episode.is_some() || !a.sub_title.trim().is_empty()) {
            return Some(Want::Episode { show: a.title.clone(), season, episode, title: a.sub_title.clone() });
        }
        Some(Want::Film { title: a.title.clone(), year: a.year })
    }

    /// The cache key: two airings that would be looked up the same way share one answer (a film
    /// repeated across the day, an episode rerun on another channel).
    pub fn key(&self) -> String {
        match self {
            Want::Film { title, year } => format!("f|{}|{}", normalise(title), year.unwrap_or(0)),
            Want::Episode { show, season, episode, title } => format!(
                "e|{}|{}|{}|{}",
                normalise(show),
                season.unwrap_or(0),
                episode.unwrap_or(0),
                if episode.is_some() { String::new() } else { normalise(title) }
            ),
        }
    }

    /// The search queries to ask, in order. An episode is asked by its own title first (that is what
    /// `/hubs/search`'s episode hub matches on), then by its show (to find the show and walk its
    /// episodes when the numbers are known).
    pub fn queries(&self) -> Vec<String> {
        match self {
            Want::Film { title, .. } => vec![title.trim().to_owned()],
            Want::Episode { show, title, .. } => {
                let mut q = Vec::new();
                if !title.trim().is_empty() {
                    q.push(title.trim().to_owned());
                }
                q.push(show.trim().to_owned());
                q
            }
        }
    }
}

/// `S02E05` / `s2e5` / `E05` → (season, episode).
pub fn parse_episode(s: &str) -> (Option<u32>, Option<u32>) {
    let s = s.trim().to_ascii_uppercase();
    let digits = |t: &str| -> Option<u32> {
        let d: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        d.parse().ok()
    };
    let season = s.strip_prefix('S').and_then(digits);
    let episode = s.find('E').and_then(|i| digits(&s[i + 1..]));
    (season, episode)
}

/// A title reduced to what two spellings of it share: lower case, `&` read as `and`, every run of
/// punctuation and space one space. `Bob's Burgers` and `Bobs Burgers` are one title; `Star Trek:
/// The Next Generation` and `Star Trek The Next Generation` too.
pub fn normalise(title: &str) -> String {
    let lowered = title.to_lowercase().replace('&', " and ");
    let mut out = String::with_capacity(lowered.len());
    let mut space = false;
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else if c != '\'' && c != '\u{2019}' {
            space = true;
        }
    }
    out
}

/// One row a search (or a show's episode list) returned, reduced to what the rules read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidate {
    pub sid: Option<ServerId>,
    pub rk: String,
    /// `movie`, `show` or `episode`.
    pub kind: String,
    pub title: String,
    pub year: i64,
    /// An episode's show (`grandparentTitle`).
    pub show: String,
    /// An episode's season (`parentIndex`) and number (`index`).
    pub season: i64,
    pub index: i64,
}

impl Candidate {
    pub fn from_metadata(sid: ServerId, m: &Metadata) -> Candidate {
        Candidate {
            sid: Some(sid),
            rk: m.rating_key.clone(),
            kind: m.kind.clone(),
            title: m.title.clone(),
            year: m.year,
            show: m.grandparent_title.clone(),
            season: m.parent_index,
            index: m.index,
        }
    }
}

/// A confident match: the item to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub sid: ServerId,
    pub rk: String,
}

/// **The match rules.** The first candidate (in the order given: servers round robin, each in its
/// own rank) that `want` confidently names, or `None`.
pub fn best(want: &Want, candidates: &[Candidate]) -> Option<Hit> {
    let hit = |c: &Candidate| c.sid.filter(|_| !c.rk.is_empty()).map(|sid| Hit { sid, rk: c.rk.clone() });
    match want {
        Want::Film { title, year } => {
            let t = normalise(title);
            let same: Vec<&Candidate> = candidates.iter().filter(|c| c.kind == "movie" && normalise(&c.title) == t).collect();
            match year {
                // The guide's year and the library's may straddle a release (a festival year, a
                // wide release) — one year either way, the exact year first.
                Some(y) => {
                    let y = i64::from(*y);
                    same.iter().find(|c| c.year == y).or_else(|| same.iter().find(|c| c.year != 0 && (c.year - y).abs() <= 1)).and_then(|c| hit(c))
                }
                // No year: only when every same-titled film is the same film (one film, or the
                // same film on two servers). Two remakes of one title are not a match.
                None => {
                    let first = same.first()?;
                    same.iter().all(|c| c.year == first.year).then(|| hit(first)).flatten()
                }
            }
        }
        Want::Episode { show, season, episode, title } => {
            let s = normalise(show);
            let episodes = candidates.iter().filter(|c| c.kind == "episode" && normalise(&c.show) == s);
            match (season, episode) {
                (Some(sn), Some(en)) => {
                    let found = episodes.clone().find(|c| c.season == i64::from(*sn) && c.index == i64::from(*en));
                    found.and_then(hit)
                }
                // An episode number without a season (`E05`): the number and the title must agree.
                (None, Some(en)) => {
                    let t = normalise(title);
                    episodes.clone().find(|c| c.index == i64::from(*en) && !t.is_empty() && normalise(&c.title) == t).and_then(hit)
                }
                _ => {
                    let t = normalise(title);
                    if t.is_empty() {
                        return None;
                    }
                    episodes.clone().find(|c| normalise(&c.title) == t).and_then(hit)
                }
            }
        }
    }
}

/// The show a search found for an episode want: same normalised title, kind `show`.
pub fn show_of<'a>(want: &Want, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    let Want::Episode { show, .. } = want else { return None };
    let s = normalise(show);
    candidates.iter().find(|c| c.kind == "show" && normalise(&c.title) == s)
}

/// Rows per hub asked of each server.
const LIMIT: i64 = 10;

/// **The blocking lookup** (a worker's): every query on every registered server until a rule
/// matches, then — for an episode with known numbers — the show's episode list.
pub fn lookup(want: &Want) -> Option<Hit> {
    let servers: Vec<ServerId> = plx_plex::plex::server_ids().collect();
    let mut candidates: Vec<Candidate> = Vec::new();
    for query in want.queries() {
        if query.chars().count() < 2 {
            continue;
        }
        for &sid in &servers {
            let Some(client) = plx_plex::plex::client_for(sid) else { continue };
            let Some(mc) = client.search(&query, LIMIT, 0) else { continue };
            for hub in &mc.hub {
                candidates.extend(hub.metadata.iter().map(|m| Candidate::from_metadata(sid, m)));
            }
        }
        if let Some(hit) = best(want, &candidates) {
            return Some(hit);
        }
    }
    // The show is in the library but the search did not surface the episode: walk its episodes.
    if let (Want::Episode { season: Some(_), episode: Some(_), .. }, Some(show)) = (want, show_of(want, &candidates)) {
        let sid = show.sid?;
        let leaves = plx_plex::plex::client_for(sid)?.all_leaves(&show.rk)?;
        let mut rows: Vec<Candidate> = leaves.metadata.iter().map(|m| Candidate::from_metadata(sid, m)).collect();
        // `allLeaves` rows of one show may omit the show title on some servers: it is implied.
        for r in &mut rows {
            if r.show.is_empty() {
                r.show = show.title.clone();
            }
        }
        return best(want, &rows);
    }
    None
}

/// What an answer was found under: the server roster and the profile. An answer from another
/// scope is stale (a switched profile may not see the library it was found in).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub roster: u32,
    pub profile: String,
}

impl Scope {
    pub fn current() -> Scope {
        Scope { roster: plx_plex::plex::server_roster_gen(), profile: plx_plex::plex::session::current_profile_key() }
    }
}

#[cfg(test)]
#[path = "plexmatch_tests.rs"]
mod tests;
