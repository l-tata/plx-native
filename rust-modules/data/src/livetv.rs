//! **Live TV** — the channels and guide of a Tunarr server, consumed exactly the way Plex and
//! Jellyfin consume one: Tunarr presents itself as an HDHomeRun tuner (`discover.json`,
//! `lineup.json`, one continuous MPEG-TS stream per channel) and publishes an XMLTV guide. Plex
//! Media Server is not in the path. The design record is `docs/live-tv-plan.md`.
//!
//! * [`hdhr`] — the HDHomeRun HTTP surface, parsed.
//! * [`xmltv`] — the guide, parsed as a stream over a window of time.
//! * [`guide`] — the two joined: [`guide::Lineup`], the model every Live TV surface reads.
//! * [`on_now`] — Home's live shelf: one card per channel, the last-tuned first.
//! * [`suggested`] — Home's Suggested Channels shelf, from the library (`crate::vchannel`).
//! * [`source`] — the blocking reads and the LAN search, run by the store's workers.
//! * [`plexmatch`] — whether an airing is in the viewer's Plex library, so the guide can offer it
//!   from the start.
//!
//! [`LiveTvState`] is the store's logical state; `stores::livetv` puts the command vocabulary and
//! the machine in front of it, like every other store. The configured server's address is an
//! install-wide preference (`Session::livetv_source`), restored at boot and written on the storage
//! worker when the person picks or forgets a server.

pub mod guide;
pub mod hdhr;
pub mod plexmatch;
pub mod on_now;
pub mod suggested;
pub mod source;
pub mod xmltv;

use guide::Lineup;
use hdhr::Device;
use plexmatch::{Hit, Scope, Want};
use source::{FetchError, Found};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

/// How long the virtual channels' guide window stands before it slides with the clock.
const VIRTUAL_SLIDE_MS: i64 = 20 * 60 * 1000;
/// The virtual channels' guide window: from this long ago…
const VIRTUAL_BEFORE_MS: i64 = 3 * 60 * 60 * 1000;
/// …to this far ahead.
const VIRTUAL_AHEAD_MS: i64 = 26 * 60 * 60 * 1000;

/// The scheme of a virtual channel's `Channel::url`: `plxvc:<playlist ratingKey>`.
pub const VIRTUAL_SCHEME: &str = "plxvc:";

/// The playlist a lineup channel's URL names, when it is a virtual channel.
pub fn virtual_playlist(url: &str) -> Option<&str> {
    url.strip_prefix(VIRTUAL_SCHEME)
}

/// A virtual channel as the guide draws it: its airings over the window around `now_ms`.
pub fn virtual_channel(vc: &crate::vchannel::channels::VChannel, now_ms: i64) -> guide::Channel {
    let mut ch = guide::Channel {
        number: vc.number(),
        name: vc.recipe.name.clone(),
        url: format!("{VIRTUAL_SCHEME}{}", vc.playlist),
        icon: String::new(),
        airings: Vec::new(),
    };
    let Some(s) = vc.schedule.as_ref() else { return ch };
    for slot in s.window(now_ms - VIRTUAL_BEFORE_MS, now_ms + VIRTUAL_AHEAD_MS) {
        let p = &s.programs()[slot.program];
        let art = if !p.art.is_empty() { &p.art } else { &p.thumb };
        ch.airings.push(guide::Airing {
            start_ms: slot.start_ms,
            stop_ms: slot.stop_ms,
            title: p.headline().to_owned(),
            sub_title: if p.episode { p.title.clone() } else { String::new() },
            desc: p.summary.clone(),
            episode: if p.episode && (p.season > 0 || p.index > 0) { format!("S{:02}E{:02}", p.season.max(0), p.index.max(0)) } else { String::new() },
            category: p.genres.first().cloned().unwrap_or_default(),
            categories: if p.episode { p.genres.clone() } else { std::iter::once("Movie".to_owned()).chain(p.genres.iter().cloned()).collect() },
            year: u16::try_from(p.year).ok().filter(|y| *y > 0),
            icon: if art.is_empty() { String::new() } else { format!("plex:{}:{}", p.sid, art) },
        });
    }
    ch
}

/// How old a loaded guide may get before entering Live TV reloads it. Tunarr rebuilds its own
/// XMLTV every 4 hours by default, so anything much faster than this re-reads the same file.
pub const STALE_MS: i64 = 30 * 60 * 1000;

/// Where the guide stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No server configured.
    Unconfigured,
    /// A load is in flight and nothing has loaded yet (a RELOAD over a loaded guide keeps `Ready`
    /// and the old guide on screen until the new one lands).
    Loading,
    Ready,
    /// The last load failed and nothing is loaded.
    Failed(FetchError),
}

/// Where the LAN search stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Discovery {
    #[default]
    Idle,
    Searching,
    /// The last search's answers (possibly none).
    Done(Vec<Found>),
}

type LoadResult = Result<(Device, Lineup, Option<FetchError>), FetchError>;

/// Whether an airing is in the viewer's Plex library ([`plexmatch`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlexMatch {
    /// A lookup is in flight.
    Looking,
    /// Not found, or not confidently: the guide offers nothing.
    None,
    Found(Hit),
}

/// Lookups in flight at once: one. A guide walk asks for a title every ~600 ms of dwell, and a
/// lookup is up to a handful of searches per server; a second worker would only race the first to
/// the same servers. A refused ask is asked again by the page while the cursor rests.
const MATCH_IN_FLIGHT: usize = 1;
/// Answers kept before the cache is emptied wholesale (a day of guide is a few hundred titles).
const MATCH_CACHE_MAX: usize = 512;

/// The Plex-match cache and its one worker.
#[derive(Default)]
struct Matches {
    scope: Option<Scope>,
    answers: HashMap<String, PlexMatch>,
    pending: Vec<(String, Receiver<Option<Hit>>)>,
}

/// The store's logical state. Workers own nothing here: they answer through a channel the pump
/// drains on the frame thread.
pub struct LiveTvState {
    source: String,
    status: Status,
    device: Option<Device>,
    /// What every surface reads: Tunarr's channels, then the profile's virtual channels.
    lineup: Arc<Lineup>,
    /// Tunarr's own channels, as loaded.
    tunarr: Arc<Lineup>,
    /// The profile's virtual channels (`crate::vchannel`).
    virtuals: crate::vchannel::channels::Channels,
    /// The start of the window the virtual channels' guide was last drawn for.
    virtual_window_ms: i64,
    guide_error: Option<FetchError>,
    loaded_at_ms: Option<i64>,
    discovery: Discovery,
    /// Bumped by every source change, so a load for a server the person has since replaced
    /// cannot land on the new one.
    epoch: u64,
    load: Option<(u64, Receiver<LoadResult>)>,
    search: Option<Receiver<Vec<Found>>>,
    matches: Matches,
    revision: u64,
}

impl Default for LiveTvState {
    fn default() -> Self {
        Self {
            source: String::new(),
            status: Status::Unconfigured,
            device: None,
            lineup: Arc::new(Lineup::default()),
            tunarr: Arc::new(Lineup::default()),
            virtuals: crate::vchannel::channels::Channels::default(),
            virtual_window_ms: 0,
            guide_error: None,
            loaded_at_ms: None,
            discovery: Discovery::Idle,
            epoch: 0,
            load: None,
            search: None,
            matches: Matches::default(),
            revision: 0,
        }
    }
}

/// The store's command vocabulary.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveTvCmd {
    /// Boot: adopt the persisted server without writing it back or loading anything yet.
    Restore { origin: String },
    /// The person picked (or typed) a server: persist it and load its guide.
    Use { origin: String },
    /// Turn Live TV off: forget the server.
    Forget,
    /// Reload the guide now.
    Refresh,
    /// Reload the guide if it is older than [`STALE_MS`], failed, or was never loaded.
    RefreshIfStale,
    /// Search the LAN for Tunarr.
    Discover,
    /// Look `want` up in the viewer's Plex library, unless it already has been (for this roster
    /// and profile) or is being.
    Match(Want),
    /// Drop everything, the configured server included (tests).
    Reset,
    /// A virtual-channel command (`crate::vchannel::channels::VCmd`).
    Virtual(crate::vchannel::channels::VCmd),
}

/// Read access, borrowed for one frame.
#[derive(Clone, Copy)]
pub struct LiveTvView<'a> {
    state: &'a LiveTvState,
}

impl<'a> LiveTvView<'a> {
    /// The configured server's origin, empty when Live TV is not set up.
    pub fn source(&self) -> &'a str {
        &self.state.source
    }
    /// Does Live TV have anything to show — a Tunarr server, a virtual channel, or a channel
    /// suggested from the library? The top strip offers the page exactly while this holds.
    pub fn configured(&self) -> bool {
        !self.state.source.is_empty() || self.has_virtuals() || !self.state.virtuals.suggestions().is_empty()
    }
    /// Does the profile hold a virtual channel?
    pub fn has_virtuals(&self) -> bool {
        !self.state.virtuals.channels().is_empty()
    }
    /// Is a Tunarr server set up (Settings > Live TV)?
    pub fn tunarr_configured(&self) -> bool {
        !self.state.source.is_empty()
    }
    /// The profile's virtual channels, their suggestions and the builder's preview.
    pub fn virtuals(&self) -> &'a crate::vchannel::channels::Channels {
        &self.state.virtuals
    }
    pub fn status(&self) -> &'a Status {
        &self.state.status
    }
    /// The joined guide. Shared, so the player can hold the channel list it is zapping through.
    pub fn lineup(&self) -> &'a Arc<Lineup> {
        &self.state.lineup
    }
    pub fn device(&self) -> Option<&'a Device> {
        self.state.device.as_ref()
    }
    /// Why the GUIDE (not the channel list) is missing, when it is.
    pub fn guide_error(&self) -> Option<&'a FetchError> {
        self.state.guide_error.as_ref()
    }
    pub fn discovery(&self) -> &'a Discovery {
        &self.state.discovery
    }
    /// Is a load in flight (first load or reload)?
    pub fn loading(&self) -> bool {
        self.state.load.is_some()
    }
    /// What is known about `key` ([`Want::key`]) in the viewer's Plex library — `None` until it is
    /// asked, and for an answer found under another roster or profile.
    pub fn plex_match(&self, key: &str) -> Option<&'a PlexMatch> {
        let m = &self.state.matches;
        if m.scope.as_ref() != Some(&Scope::current()) {
            return None;
        }
        m.answers.get(key)
    }
    /// Is `hit` an answer this store found (for the current roster and profile)? What the loop
    /// checks before it opens an item a page asked for, so a stale or forged request opens nothing.
    pub fn knows_match(&self, hit: &Hit) -> bool {
        let m = &self.state.matches;
        m.scope.as_ref() == Some(&Scope::current()) && m.answers.values().any(|a| *a == PlexMatch::Found(hit.clone()))
    }
    /// Moves whenever anything above moves.
    pub fn revision(&self) -> u64 {
        self.state.revision
    }
}

impl LiveTvState {
    pub fn view(&self) -> LiveTvView<'_> {
        LiveTvView { state: self }
    }

    /// Apply one command at `now_ms` (wall time). `true` when anything observable changed.
    pub fn run(&mut self, cmd: LiveTvCmd, now_ms: i64) -> bool {
        let changed = match cmd {
            LiveTvCmd::Restore { origin } => {
                let origin = hdhr::normalise_address(&origin).unwrap_or_default();
                self.set_source(origin)
            }
            LiveTvCmd::Use { origin } => {
                let Some(origin) = hdhr::normalise_address(&origin) else { return false };
                persist(&origin);
                // Always a change: even re-picking the same server starts a fresh load and closes
                // the search results the pick was made from.
                self.set_source(origin);
                self.discovery = Discovery::Idle;
                self.start_load();
                true
            }
            LiveTvCmd::Forget => {
                if self.source.is_empty() {
                    return false;
                }
                persist("");
                self.set_source(String::new())
            }
            LiveTvCmd::Refresh => self.start_load(),
            LiveTvCmd::RefreshIfStale => {
                // "Already loading" is a worker in flight (`start_load` refuses a second), never
                // the status alone: Restore marks a server Loading without fetching it.
                let stale = match (&self.status, self.loaded_at_ms) {
                    (Status::Ready, Some(at)) => now_ms - at >= STALE_MS || now_ms < at,
                    _ => true,
                };
                stale && self.start_load()
            }
            LiveTvCmd::Discover => self.start_search(),
            LiveTvCmd::Match(want) => self.start_match(want),
            LiveTvCmd::Reset => {
                *self = LiveTvState { revision: self.revision, epoch: self.epoch + 1, ..LiveTvState::default() };
                true
            }
            LiveTvCmd::Virtual(cmd) => {
                let changed = self.virtuals.run(cmd, now_ms);
                if changed {
                    self.republish(now_ms);
                }
                changed
            }
        };
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// Publish Tunarr's channels followed by the virtual channels, whose airings are drawn from
    /// their timelines over a window around `now_ms`.
    fn republish(&mut self, now_ms: i64) {
        self.virtual_window_ms = now_ms;
        if self.virtuals.channels().is_empty() {
            self.lineup = Arc::clone(&self.tunarr);
            return;
        }
        let mut merged = (*self.tunarr).clone();
        let taken: std::collections::HashSet<String> = merged.channels.iter().map(|c| c.number.clone()).collect();
        for vc in self.virtuals.channels() {
            if taken.contains(&vc.number()) {
                continue; // a Tunarr channel already has that number; the virtual one yields
            }
            merged.channels.push(virtual_channel(vc, now_ms));
        }
        self.lineup = Arc::new(merged);
    }

    fn set_source(&mut self, origin: String) -> bool {
        if origin == self.source {
            return false;
        }
        self.epoch += 1;
        self.load = None;
        self.source = origin;
        self.device = None;
        self.tunarr = Arc::new(Lineup::default());
        self.republish(plx_base::wallclock::now_ms());
        self.guide_error = None;
        self.loaded_at_ms = None;
        self.status = if self.source.is_empty() { Status::Unconfigured } else { Status::Loading };
        true
    }

    fn start_load(&mut self) -> bool {
        if self.source.is_empty() || self.load.is_some() || crate::stores::tape::active() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let origin = self.source.clone();
        let spawned = plx_base::task::spawn_small("livetv-load", move || {
            let _ = tx.send(source::load(&origin, plx_base::wallclock::now_ms()));
            plx_machine::idle::wake();
        });
        if !spawned {
            return false;
        }
        self.load = Some((self.epoch, rx));
        if !matches!(self.status, Status::Ready) {
            self.status = Status::Loading;
        }
        true
    }

    fn start_search(&mut self) -> bool {
        if self.search.is_some() || crate::stores::tape::active() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = plx_base::task::spawn_small("livetv-search", move || {
            let _ = tx.send(source::discover());
            plx_machine::idle::wake();
        });
        if !spawned {
            return false;
        }
        self.search = Some(rx);
        self.discovery = Discovery::Searching;
        true
    }

    fn start_match(&mut self, want: Want) -> bool {
        let scope = Scope::current();
        let m = &mut self.matches;
        if m.scope.as_ref() != Some(&scope) {
            // Another roster or profile: every answer (and every lookup in flight) is stale.
            m.scope = Some(scope);
            m.answers.clear();
            m.pending.clear();
        }
        let key = want.key();
        if m.answers.contains_key(&key) || m.pending.len() >= MATCH_IN_FLIGHT || crate::stores::tape::active() {
            return false;
        }
        if m.answers.len() >= MATCH_CACHE_MAX {
            m.answers.retain(|_, v| *v == PlexMatch::Looking);
        }
        let (tx, rx) = mpsc::channel();
        let spawned = plx_base::task::spawn_small("livetv-plexmatch", move || {
            let _ = tx.send(plexmatch::lookup(&want));
            plx_machine::idle::wake();
        });
        if !spawned {
            return false;
        }
        m.answers.insert(key.clone(), PlexMatch::Looking);
        m.pending.push((key, rx));
        true
    }

    /// Land the Plex-match lookups that finished. `true` when an answer arrived.
    fn pump_matches(&mut self) -> bool {
        let mut changed = false;
        let m = &mut self.matches;
        m.pending.retain(|(key, rx)| match rx.try_recv() {
            Ok(found) => {
                m.answers.insert(key.clone(), found.map_or(PlexMatch::None, PlexMatch::Found));
                changed = true;
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                m.answers.insert(key.clone(), PlexMatch::None);
                changed = true;
                false
            }
        });
        changed
    }

    /// Answer `want` without a worker (tests).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_match_for_test(&mut self, want: &Want, answer: PlexMatch) {
        self.matches.scope = Some(Scope::current());
        self.matches.answers.insert(want.key(), answer);
        self.revision += 1;
    }

    /// Drain the workers. `true` when a landing changed anything.
    pub fn pump(&mut self, now_ms: i64) -> bool {
        let mut changed = false;
        if let Some((epoch, rx)) = self.load.as_ref() {
            match rx.try_recv() {
                Ok(result) => {
                    let current = *epoch == self.epoch;
                    self.load = None;
                    if current {
                        self.land(result, now_ms);
                        changed = true;
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.load = None;
                    if !matches!(self.status, Status::Ready) {
                        self.status = Status::Failed(FetchError::Unreachable);
                    }
                    changed = true;
                }
            }
        }
        if let Some(rx) = self.search.as_ref() {
            match rx.try_recv() {
                Ok(found) => {
                    self.search = None;
                    self.discovery = Discovery::Done(found);
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.search = None;
                    self.discovery = Discovery::Done(Vec::new());
                    changed = true;
                }
            }
        }
        changed |= self.pump_matches();
        let virtual_changed = self.virtuals.pump(now_ms);
        // The virtual channels' guide is drawn for a window; slide it as time passes.
        if virtual_changed || (!self.virtuals.channels().is_empty() && now_ms - self.virtual_window_ms >= VIRTUAL_SLIDE_MS) {
            self.republish(now_ms);
            changed = true;
        }
        if changed {
            self.revision += 1;
            plx_machine::idle::invalidate();
        }
        changed
    }

    fn land(&mut self, result: LoadResult, now_ms: i64) {
        match result {
            Ok((device, lineup, guide_error)) => {
                plx_base::eventlog::log(&format!(
                    "livetv: loaded channels={} airings={} guide={}",
                    lineup.len(),
                    lineup.channels.iter().map(|c| c.airings.len()).sum::<usize>(),
                    guide_error.as_ref().map_or_else(|| "ok".to_owned(), ToString::to_string),
                ));
                self.device = Some(device);
                plx_base::wallclock::set_offset_hint(lineup.source_offset_s);
                self.tunarr = Arc::new(lineup);
                self.republish(now_ms);
                self.guide_error = guide_error;
                self.loaded_at_ms = Some(now_ms);
                self.status = Status::Ready;
            }
            Err(e) => {
                plx_base::eventlog::log(&format!("livetv: load failed ({e})"));
                // A failed RELOAD keeps the guide that is on screen.
                if !matches!(self.status, Status::Ready) {
                    self.status = Status::Failed(e);
                }
            }
        }
    }

    /// Install a loaded guide directly (tests and the simulator's fixtures).
    #[cfg(any(test, feature = "test-support"))]
    pub fn install_for_test(&mut self, origin: &str, lineup: Lineup, now_ms: i64) {
        self.source = origin.to_owned();
        self.tunarr = Arc::new(lineup);
        self.republish(now_ms);
        self.status = Status::Ready;
        self.loaded_at_ms = Some(now_ms);
        self.revision += 1;
    }

    /// Install virtual channels and suggestions directly, and publish them in the lineup.
    #[cfg(any(test, feature = "test-support"))]
    pub fn install_virtuals_for_test(
        &mut self,
        channels: Vec<crate::vchannel::channels::VChannel>,
        suggestions: Vec<crate::vchannel::suggest::Suggestion>,
        now_ms: i64,
    ) {
        self.virtuals.install_for_test(channels);
        self.virtuals.install_suggestions_for_test(suggestions);
        self.republish(now_ms);
        self.revision += 1;
    }

    /// Land a load result as if a worker had returned it (tests).
    #[cfg(test)]
    fn land_for_test(&mut self, result: LoadResult, now_ms: i64) {
        self.land(result, now_ms);
        self.revision += 1;
    }
}

/// Write the configured server to the session on the storage worker: the frame must not block on
/// storage, and the in-memory value has already moved (a failed write costs the setting at the
/// next boot, never the session).
/// Remember `number` as the channel last tuned (`Session::livetv_channel`), so the guide opens on
/// it next time. Written on the storage worker; nothing to do when it is already the saved one.
pub fn remember_channel(number: &str) {
    if crate::stores::tape::active() || cfg!(test) || number.is_empty()
        || plx_plex::plex::session::peek().livetv_channel() == number
    {
        return;
    }
    let number = number.to_owned();
    let _ = plx_base::storage_worker::submit_retained(move || {
        if plx_plex::plex::session::update_with_outcome(|s| Some(s.with_livetv_channel(&number))).is_none() {
            plx_base::eventlog::log("livetv: the last channel was not saved");
        }
    });
}

fn persist(origin: &str) {
    if crate::stores::tape::active() || cfg!(test) {
        return;
    }
    let origin = origin.to_owned();
    let _ = plx_base::storage_worker::submit_retained(move || {
        let durable = plx_plex::plex::session::update_with_outcome(|s| Some(s.with_livetv_source(&origin)))
            .is_some_and(|write| matches!(write.classify(),
                plx_plex::plex::session::async_persistence::CompletionOutcome::Durable(_)));
        if !durable {
            plx_base::eventlog::log("livetv: the Live TV server was not saved");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guide::Channel;

    fn lineup(n: usize) -> Lineup {
        Lineup { channels: (0..n).map(|i| Channel { number: (i + 1).to_string(), ..Default::default() }).collect(), ..Default::default() }
    }

    #[test]
    fn restore_adopts_a_normalised_source_without_loading() {
        let mut s = LiveTvState::default();
        assert!(s.run(LiveTvCmd::Restore { origin: "192.0.2.20".into() }, 0));
        assert_eq!(s.view().source(), "http://192.0.2.20:8000");
        assert!(s.view().configured());
        assert!(!s.view().loading(), "boot does not fetch a guide nobody asked for");
        assert!(!s.run(LiveTvCmd::Restore { origin: "http://192.0.2.20:8000".into() }, 0), "same source, no change");
        assert!(s.run(LiveTvCmd::Restore { origin: "not a host".into() }, 0), "garbage restores as unset");
        assert_eq!(*s.view().status(), Status::Unconfigured);
    }

    /// **A restored server loads when the page asks.** Restore marks the guide Loading without
    /// starting a worker (boot fetches nothing), so "already loading" must mean a worker is in
    /// flight, not the status: otherwise the page's RefreshIfStale did nothing and the guide sat on
    /// "Loading the guide" for ever.
    #[test]
    fn refresh_if_stale_loads_a_restored_server() {
        let mut s = LiveTvState::default();
        // A port that refuses at once, so the real worker `cfg(test)` spawns ends immediately.
        s.run(LiveTvCmd::Restore { origin: "http://127.0.0.1:9".into() }, 0);
        assert!(!s.view().loading());
        assert!(s.run(LiveTvCmd::RefreshIfStale, 0), "the restored server is fetched");
        assert!(s.view().loading());
        assert!(!s.run(LiveTvCmd::RefreshIfStale, 0), "and only once while that fetch is in flight");
    }

    #[test]
    fn a_landing_installs_the_guide_and_a_failed_reload_keeps_it() {
        let mut s = LiveTvState::default();
        s.run(LiveTvCmd::Restore { origin: "192.0.2.20".into() }, 0);
        s.land_for_test(Ok((Device::default(), lineup(3), None)), 1000);
        assert_eq!(*s.view().status(), Status::Ready);
        assert_eq!(s.view().lineup().len(), 3);
        s.land_for_test(Err(FetchError::Unreachable), 2000);
        assert_eq!(*s.view().status(), Status::Ready, "a failed reload keeps the guide on screen");
        assert_eq!(s.view().lineup().len(), 3);
    }

    #[test]
    fn a_first_load_failure_is_reported() {
        let mut s = LiveTvState::default();
        s.run(LiveTvCmd::Restore { origin: "192.0.2.20".into() }, 0);
        s.land_for_test(Err(FetchError::Status(404)), 0);
        assert_eq!(*s.view().status(), Status::Failed(FetchError::Status(404)));
    }

    #[test]
    fn staleness_is_judged_on_the_wall_clock() {
        let mut s = LiveTvState::default();
        s.install_for_test("http://192.0.2.20:8000", lineup(1), 1_000_000);
        let rev = s.view().revision();
        // `cfg(test)` workers are real threads; `RefreshIfStale` on a fresh guide must not start one.
        assert!(!s.run(LiveTvCmd::RefreshIfStale, 1_000_000 + STALE_MS - 1));
        assert_eq!(s.view().revision(), rev);
        assert!(!s.view().loading());
    }

    #[test]
    fn forgetting_clears_the_guide() {
        let mut s = LiveTvState::default();
        s.install_for_test("http://192.0.2.20:8000", lineup(2), 0);
        assert!(s.run(LiveTvCmd::Forget, 0));
        assert!(!s.view().configured());
        assert!(s.view().lineup().is_empty());
        assert_eq!(*s.view().status(), Status::Unconfigured);
        assert!(!s.run(LiveTvCmd::Forget, 0), "nothing left to forget");
    }

    #[test]
    fn a_plex_match_is_asked_once_and_lands_through_the_pump() {
        let _g = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        let mut s = LiveTvState::default();
        let want = Want::Film { title: "Sintel".into(), year: None };
        assert_eq!(s.view().plex_match(&want.key()), None, "never asked");
        assert!(s.run(LiveTvCmd::Match(want.clone()), 0));
        assert_eq!(s.view().plex_match(&want.key()), Some(&PlexMatch::Looking));
        assert!(!s.run(LiveTvCmd::Match(want.clone()), 0), "asked once");
        let other = Want::Film { title: "Big Buck Bunny".into(), year: None };
        assert!(!s.run(LiveTvCmd::Match(other.clone()), 0), "one lookup in flight at a time");
        // No servers are registered, so the real worker answers "not found" at once.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !s.pump(0) {
            assert!(std::time::Instant::now() < deadline, "the lookup never landed");
            std::thread::yield_now();
        }
        assert_eq!(s.view().plex_match(&want.key()), Some(&PlexMatch::None));
        assert!(s.run(LiveTvCmd::Match(other), 0), "the slot is free again");
    }

    #[test]
    fn a_plex_answer_from_another_profile_is_not_shown() {
        let mut s = LiveTvState::default();
        let want = Want::Film { title: "Sintel".into(), year: None };
        s.set_match_for_test(&want, PlexMatch::Found(Hit { sid: plx_plex::plex::ServerId::from_raw(0), rk: "1".into() }));
        assert!(matches!(s.view().plex_match(&want.key()), Some(PlexMatch::Found(_))));
        let hit = Hit { sid: plx_plex::plex::ServerId::from_raw(0), rk: "1".into() };
        assert!(s.view().knows_match(&hit));
        assert!(!s.view().knows_match(&Hit { rk: "2".into(), ..hit.clone() }), "only what was found");
        s.matches.scope = Some(Scope { roster: u32::MAX, profile: "someone-else".into() });
        assert_eq!(s.view().plex_match(&want.key()), None);
        assert!(!s.view().knows_match(&hit), "nor under another profile");
    }

    #[test]
    fn an_unusable_typed_address_changes_nothing() {
        let mut s = LiveTvState::default();
        assert!(!s.run(LiveTvCmd::Use { origin: "  ".into() }, 0));
        assert!(!s.view().configured());
    }
}
