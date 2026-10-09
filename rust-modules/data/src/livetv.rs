//! **Live TV** — the channels and guide of a Tunarr server, consumed exactly the way Plex and
//! Jellyfin consume one: Tunarr presents itself as an HDHomeRun tuner (`discover.json`,
//! `lineup.json`, one continuous MPEG-TS stream per channel) and publishes an XMLTV guide. Plex
//! Media Server is not in the path. The design record is `docs/live-tv-plan.md`.
//!
//! * [`hdhr`] — the HDHomeRun HTTP surface, parsed.
//! * [`xmltv`] — the guide, parsed as a stream over a window of time.
//! * [`guide`] — the two joined: [`guide::Lineup`], the model every Live TV surface reads.
//! * [`source`] — the blocking reads and the LAN search, run by the store's workers.
//!
//! [`LiveTvState`] is the store's logical state; `stores::livetv` puts the command vocabulary and
//! the machine in front of it, like every other store. The configured server's address is an
//! install-wide preference (`Session::livetv_source`), restored at boot and written on the storage
//! worker when the person picks or forgets a server.

pub mod guide;
pub mod hdhr;
pub mod source;
pub mod xmltv;

use guide::Lineup;
use hdhr::Device;
use source::{FetchError, Found};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

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

/// The store's logical state. Workers own nothing here: they answer through a channel the pump
/// drains on the frame thread.
pub struct LiveTvState {
    source: String,
    status: Status,
    device: Option<Device>,
    lineup: Arc<Lineup>,
    guide_error: Option<FetchError>,
    loaded_at_ms: Option<i64>,
    discovery: Discovery,
    /// Bumped by every source change, so a load for a server the person has since replaced
    /// cannot land on the new one.
    epoch: u64,
    load: Option<(u64, Receiver<LoadResult>)>,
    search: Option<Receiver<Vec<Found>>>,
    revision: u64,
}

impl Default for LiveTvState {
    fn default() -> Self {
        Self {
            source: String::new(),
            status: Status::Unconfigured,
            device: None,
            lineup: Arc::new(Lineup::default()),
            guide_error: None,
            loaded_at_ms: None,
            discovery: Discovery::Idle,
            epoch: 0,
            load: None,
            search: None,
            revision: 0,
        }
    }
}

/// The store's command vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Drop everything, the configured server included (tests).
    Reset,
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
    pub fn configured(&self) -> bool {
        !self.state.source.is_empty()
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
            LiveTvCmd::Reset => {
                *self = LiveTvState { revision: self.revision, epoch: self.epoch + 1, ..LiveTvState::default() };
                true
            }
        };
        if changed {
            self.revision += 1;
        }
        changed
    }

    fn set_source(&mut self, origin: String) -> bool {
        if origin == self.source {
            return false;
        }
        self.epoch += 1;
        self.load = None;
        self.source = origin;
        self.device = None;
        self.lineup = Arc::new(Lineup::default());
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
                self.lineup = Arc::new(lineup);
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
        self.lineup = Arc::new(lineup);
        self.status = Status::Ready;
        self.loaded_at_ms = Some(now_ms);
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
    fn an_unusable_typed_address_changes_nothing() {
        let mut s = LiveTvState::default();
        assert!(!s.run(LiveTvCmd::Use { origin: "  ".into() }, 0));
        assert!(!s.view().configured());
    }
}
