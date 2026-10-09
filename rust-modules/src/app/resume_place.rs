//! **Reopen where you left off.** The page the app was on — Home, a library, Search, Live TV, or
//! an item's page — is written down as it changes, and a cold launch that lands on Home goes back
//! there before the viewer has pressed anything.
//!
//! A playback is remembered as the PLAYED ITEM's page (an episode's, not its show's), never as the
//! player: a power cycle that came back into a decoding video would be an ambush, and the resume
//! position is the server's `viewOffset`, which that page's Resume button already offers. A Live TV
//! channel is remembered as the Live TV page, which opens on the channel last tuned.
//!
//! The upstream project shipped this once (#56, `coldstart.rs`) and retired it on 2026-09-01 in
//! favour of always starting on Home; this fork brings it back by its owner's choice. The retired
//! file (`lastplace.json`) is still swept by `coldstart::retire`; this one is `resume.json`
//! (`paths::resume_place_candidates`), beside the session file, because `/tmp` does not survive
//! the power cycle a resume point exists for.
//!
//! What it will not do: restore across profiles (the place is keyed to the profile that left it),
//! restore a place older than [`FRESH_FOR_S`], override a dev boot trigger, or move a viewer who
//! has already pressed a key.

use super::App;
use plx_screens::registry::{AppArg, ContentArg};
use std::path::PathBuf;

/// How old a place may be and still be reopened: a viewer back the next morning wants Home.
const FRESH_FOR_S: i64 = 12 * 60 * 60;
/// How long after boot the restore may still happen (an item's server has to be discovered first).
const RESTORE_WINDOW_MS: u32 = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Home,
    Library(plx_data::browse::SecKind),
    Search,
    LiveTv,
    /// An item's page, by its server's machine identifier (a `ServerId` is a slot number that a
    /// later launch may hand to another server).
    Item { machine: String, rk: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub place: Place,
    /// `session::current_profile_key()` when it was written.
    pub profile: String,
    /// Unix seconds.
    pub at: i64,
}

impl Record {
    pub(crate) fn to_json(&self) -> String {
        let mut o = serde_json::json!({ "v": 1, "profile": self.profile, "at": self.at });
        let (kind, extra): (&str, Option<(&str, String)>) = match &self.place {
            Place::Home => ("home", None),
            Place::Library(k) => ("library", Some(("section", section_word(*k).to_owned()))),
            Place::Search => ("search", None),
            Place::LiveTv => ("livetv", None),
            Place::Item { machine, rk } => {
                o["machine"] = machine.clone().into();
                ("item", Some(("rk", rk.clone())))
            }
        };
        o["kind"] = kind.into();
        if let Some((k, v)) = extra {
            o[k] = v.into();
        }
        o.to_string()
    }

    /// `None` for anything unreadable: a stale convenience file never stands between a viewer
    /// and Home.
    pub(crate) fn from_json(raw: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(raw).ok()?;
        if v.get("v")?.as_u64()? != 1 {
            return None;
        }
        let text = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(str::to_owned);
        let place = match v.get("kind")?.as_str()? {
            "home" => Place::Home,
            "library" => Place::Library(section_of(&text("section")?)?),
            "search" => Place::Search,
            "livetv" => Place::LiveTv,
            "item" => {
                let (machine, rk) = (text("machine")?, text("rk")?);
                if machine.is_empty() || rk.is_empty() {
                    return None;
                }
                Place::Item { machine, rk }
            }
            _ => return None,
        };
        Some(Self { place, profile: text("profile").unwrap_or_default(), at: v.get("at")?.as_i64()? })
    }
}

fn section_word(k: plx_data::browse::SecKind) -> &'static str {
    match k {
        plx_data::browse::SecKind::Show => "show",
        _ => "movie",
    }
}

fn section_of(w: &str) -> Option<plx_data::browse::SecKind> {
    match w {
        "movie" => Some(plx_data::browse::SecKind::Movie),
        "show" => Some(plx_data::browse::SecKind::Show),
        _ => None,
    }
}

/// Should this record be reopened, for `profile` at `now` (both Unix seconds)? Pure: the decision
/// [`pump`] makes before it navigates.
pub(crate) fn worth_reopening(record: &Record, profile: &str, now: i64) -> bool {
    record.place != Place::Home
        && record.profile == profile
        && now >= record.at
        && now - record.at <= FRESH_FOR_S
}

/// The place a page argument names, `None` for a page that is not a place to come back to (a
/// sign-in, Settings, a person page, the player — whose item the caller supplies instead).
pub(crate) fn place_of(arg: &AppArg, library: Option<plx_data::browse::SecKind>, machine_of: impl Fn(plx_plex::plex::ServerId) -> Option<String>) -> Option<Place> {
    match arg {
        AppArg::Home => Some(Place::Home),
        AppArg::Library => library.map(Place::Library),
        AppArg::Search => Some(Place::Search),
        AppArg::LiveTv => Some(Place::LiveTv),
        AppArg::Content(ContentArg::Detail { sid, rk }) => {
            machine_of(*sid).map(|machine| Place::Item { machine, rk: rk.clone() })
        }
        _ => None,
    }
}

/// The tracker: what was last written, and the one restore a launch may perform.
pub(crate) struct ResumePlace {
    written: Option<Place>,
    pending: Option<Record>,
    boot_input: u32,
}

impl Default for ResumePlace {
    fn default() -> Self {
        Self { written: None, pending: None, boot_input: u32::MAX }
    }
}

fn now_s() -> i64 {
    plx_base::wallclock::now_ms() / 1000
}

fn machine_of(sid: plx_plex::plex::ServerId) -> Option<String> {
    plx_plex::plex::client_for(sid).map(|c| c.machine_id().to_owned()).filter(|m| !m.is_empty())
}

/// At boot: read the record, if any, and arm it. A dev boot trigger that opens a page wins.
pub(crate) fn begin(app: &mut App) {
    app.resume.boot_input = app.last_input;
    let dev_opens_a_page = ["detail", "library", "search", "play", "playidx", "collection", "settings", "livetv"]
        .iter()
        .any(|t| plx_base::devtrig::read(t).is_some());
    if dev_opens_a_page {
        return;
    }
    let record = plx_base::paths::resume_place_candidates()
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| Record::from_json(&raw));
    if let Some(record) = record {
        if worth_reopening(&record, &plx_plex::plex::session::current_profile_key(), now_s()) {
            app.resume.pending = Some(record);
        }
    }
}

/// Once a frame: perform the armed restore when it can be, and write the place when it changes.
pub(crate) fn pump(app: &mut App, now_ms: u32) {
    restore(app, now_ms);
    track(app);
}

fn restore(app: &mut App, now_ms: u32) {
    let Some(record) = app.resume.pending.as_ref() else { return };
    let late = now_ms.wrapping_sub(app.t0) > RESTORE_WINDOW_MS;
    let touched = app.last_input != app.resume.boot_input;
    if late || touched || app.route() != AppArg::Home && app.route() != AppArg::Library {
        if late || touched {
            app.resume.pending = None;
        }
        return;
    }
    let done = match &record.place {
        Place::Home => true,
        Place::Library(kind) => {
            let tab = match kind {
                plx_data::browse::SecKind::Show => plx_screens::registry::HomeTab::Shows,
                _ => plx_screens::registry::HomeTab::Movies,
            };
            if app.bridge.search_tab_available(tab) {
                app.bridge.enter_library(*kind);
                super::bridge::nav_select_tab(&mut app.pages, AppArg::Library);
                true
            } else {
                false
            }
        }
        Place::Search => {
            super::bridge::nav_select_tab(&mut app.pages, AppArg::Search);
            true
        }
        Place::LiveTv => {
            if app.bridge.livetv_view().configured() {
                super::livetv::open_page(&mut app.pages, &mut app.bridge, false);
                true
            } else {
                false
            }
        }
        Place::Item { machine, rk } => match plx_plex::plex::id_of_machine(machine) {
            Some(sid) => {
                let rk = rk.clone();
                app.bridge.metadata_mut().run(plx_data::stores::metadata::MetadataCmd::RequestDetail { sid, rk: rk.clone() });
                super::bridge::open_detail(&mut app.pages, &mut app.bridge, sid, &rk, None, None);
                true
            }
            None => false,
        },
    };
    if done {
        plx_base::eventlog::log("resume: reopened where the last session left off");
        app.resume.pending = None;
    }
}

fn track(app: &mut App) {
    // The player is its item; a channel is the Live TV page.
    let arg = app.route();
    let place = if arg == AppArg::Player {
        if plx_media::route::live(&app.player.session).is_some() {
            Some(Place::LiveTv)
        } else {
            let rk = plx_media::route::cur_rk(&app.player.session);
            machine_of(plx_media::route::cur_sid(&app.player.session))
                .filter(|_| !rk.is_empty())
                .map(|machine| Place::Item { machine, rk })
        }
    } else {
        place_of(&arg, app.bridge.library_kind(), machine_of)
    };
    let Some(place) = place else { return };
    if app.resume.written.as_ref() == Some(&place) {
        return;
    }
    // Nothing is written while a restore is still armed: Home on the first frames is where the
    // launch landed, not where the viewer chose to be.
    if app.resume.pending.is_some() {
        return;
    }
    app.resume.written = Some(place.clone());
    let record = Record { place, profile: plx_plex::plex::session::current_profile_key(), at: now_s() };
    persist(record.to_json());
}

fn persist(json: String) {
    if cfg!(test) {
        return;
    }
    let _ = plx_base::storage_worker::submit_retained(move || {
        let written = plx_base::paths::resume_place_candidates().into_iter().any(|path| write_atomic(&path, &json));
        if !written {
            plx_base::eventlog::log("resume: the place could not be written");
        }
    });
}

fn write_atomic(path: &PathBuf, body: &str) -> bool {
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(place: Place, profile: &str, at: i64) -> Record {
        Record { place, profile: profile.into(), at }
    }

    #[test]
    fn a_record_round_trips_through_its_file() {
        for place in [
            Place::Home,
            Place::Library(plx_data::browse::SecKind::Show),
            Place::Search,
            Place::LiveTv,
            Place::Item { machine: "aaaabbbbcccc".into(), rk: "42".into() },
        ] {
            let r = rec(place, "uuid-1", 1_791_500_000);
            assert_eq!(Record::from_json(&r.to_json()), Some(r));
        }
    }

    #[test]
    fn an_unreadable_or_foreign_file_is_ignored() {
        for raw in ["", "not json", r#"{"v":2,"kind":"home","at":1}"#, r#"{"v":1,"kind":"item","machine":"","rk":"1","at":1}"#, r#"{"v":1,"kind":"elsewhere","at":1}"#] {
            assert_eq!(Record::from_json(raw), None, "{raw}");
        }
    }

    #[test]
    fn only_a_fresh_place_of_the_same_profile_is_reopened() {
        let item = Place::Item { machine: "m".into(), rk: "1".into() };
        let now = 1_791_500_000;
        assert!(worth_reopening(&rec(item.clone(), "p", now - 60), "p", now));
        assert!(!worth_reopening(&rec(item.clone(), "p", now - FRESH_FOR_S - 1), "p", now), "too old");
        assert!(!worth_reopening(&rec(item.clone(), "other", now - 60), "p", now), "another profile's place");
        assert!(!worth_reopening(&rec(item, "p", now + 600), "p", now), "a clock that moved backwards");
        assert!(!worth_reopening(&rec(Place::Home, "p", now - 60), "p", now), "Home is where a launch lands anyway");
    }

    #[test]
    fn pages_map_to_places_and_others_do_not() {
        let sid = plx_plex::plex::ServerId::UNSET;
        let machine = |_| Some("m".to_owned());
        assert_eq!(place_of(&AppArg::Home, None, machine), Some(Place::Home));
        assert_eq!(place_of(&AppArg::Library, Some(plx_data::browse::SecKind::Movie), machine), Some(Place::Library(plx_data::browse::SecKind::Movie)));
        assert_eq!(place_of(&AppArg::Library, None, machine), None, "no library entered yet");
        assert_eq!(
            place_of(&AppArg::Content(ContentArg::Detail { sid, rk: "7".into() }), None, machine),
            Some(Place::Item { machine: "m".into(), rk: "7".into() })
        );
        assert_eq!(place_of(&AppArg::Content(ContentArg::Detail { sid, rk: "7".into() }), None, |_| None), None, "an unknown server");
        assert_eq!(place_of(&AppArg::Player, None, machine), None);
        assert_eq!(place_of(&AppArg::Login, None, machine), None);
    }
}
