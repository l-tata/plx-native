//! **The virtual channels the profile holds**, live: their recipes (listed from the server), their
//! built timelines, the library catalog they and the suggestions read, the builder's preview, and
//! the writes in flight. Owned by the Live TV store beside the Tunarr guide (`crate::livetv`),
//! which merges these channels into the lineup every Live TV surface reads.
//!
//! Workers are plain threads answering through channels, drained by [`Channels::pump`] on the
//! frame thread — the Live TV store's own pattern — and, like it, none starts under an active tape.
//! Everything is scoped to the server and profile it was read for ([`Scope`]); a profile switch
//! drops it all.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use super::build::{programmes, PROGRAMMES_MAX};
use super::catalog::Catalog;
use super::recipe::{next_number, Recipe};
use super::remote::{self, ClientReads, Stored};
use super::schedule::{Program, Schedule};
use super::suggest::{self, Moment, Suggestion};
use super::dismissed;

/// How long the channel list is trusted before entering Live TV or Home reads it again (another
/// television may have added or deleted a channel).
pub const LIST_STALE_MS: i64 = 5 * 60 * 1000;
/// How long the catalog and a channel's programmes are trusted (new episodes join the next pass
/// after a rebuild lands).
pub const CATALOG_STALE_MS: i64 = 30 * 60 * 1000;
/// Builds in flight at once.
const BUILDS_IN_FLIGHT: usize = 2;
/// How long a row of suggestions stands before it is worked out again (the hour of the day and
/// the day of the week are part of the score).
pub const SUGGEST_STALE_MS: i64 = 60 * 60 * 1000;

/// Which server and profile the state was read for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub sid: u16,
    pub profile: String,
}

impl Scope {
    pub fn current() -> Scope {
        Scope { sid: plx_plex::plex::current_server().raw(), profile: plx_plex::plex::session::current_profile_key() }
    }
}

/// One channel the profile holds.
#[derive(Clone, Debug)]
pub struct VChannel {
    /// Its playlist on the server — the channel's identity.
    pub playlist: String,
    pub recipe: Recipe,
    pub composite: String,
    /// The built timeline; `None` until the first build lands.
    pub schedule: Option<Schedule>,
    pub built_at_ms: Option<i64>,
    /// The last build could not read the source (the channel keeps any timeline it had).
    pub failed: bool,
}

impl VChannel {
    pub fn number(&self) -> String {
        self.recipe.number.to_string()
    }
}

/// The builder's draft and the timeline it would air.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The caller's token for the draft, so a page can tell its own preview from a stale one.
    pub token: u64,
    pub recipe: Recipe,
    pub schedule: Option<Schedule>,
    pub failed: bool,
}

/// What the list stands at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListState {
    #[default]
    Unread,
    Reading,
    Ready,
    Failed,
}

/// A write's outcome, kept for the page that asked (the builder closes on a kept channel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteDone {
    Kept { token: u64, playlist: String },
    Updated { playlist: String },
    Deleted { playlist: String },
    Failed { token: u64 },
}

/// The command vocabulary (carried by `LiveTvCmd::Virtual`).
#[derive(Clone, Debug, PartialEq)]
pub enum VCmd {
    /// Read the channel list (and the catalog if stale).
    Refresh,
    /// [`VCmd::Refresh`] if the list is older than [`LIST_STALE_MS`] or was never read.
    RefreshIfStale,
    /// Build `recipe`'s timeline for the builder, tagged `token`.
    Preview { token: u64, recipe: Recipe },
    ClearPreview,
    /// Keep the previewed draft (or `recipe`) as a channel: number it and create its playlist.
    Keep { token: u64, recipe: Recipe },
    /// Write a changed recipe to its channel (a rename, a new order, new rules); `rebuild` reads
    /// its programmes again.
    Update { playlist: String, recipe: Recipe, rebuild: bool },
    Delete { playlist: String },
    /// Work the suggestions out again now.
    Suggest,
    /// The profile is not interested in suggestion `id`: never show it again.
    Dismiss { id: String },
    /// Draw a surprise channel (seeded, so the same press draws the same idea).
    Surprise { seed: u64 },
}

enum Job {
    List(Receiver<Option<Vec<Stored>>>),
    Catalog(Receiver<Option<Catalog>>),
    Build { playlist: String, rx: Receiver<Option<Vec<Program>>> },
    Preview { token: u64, rx: Receiver<Option<Vec<Program>>> },
    Write(Receiver<(WriteDone, Option<Stored>)>),
    Suggest(Receiver<Vec<Suggestion>>),
    Surprise(Receiver<Option<Suggestion>>),
}

/// The state. See the module doc.
#[derive(Default)]
pub struct Channels {
    scope: Option<Scope>,
    list: ListState,
    listed_at: Option<i64>,
    channels: Vec<VChannel>,
    catalog: Option<Arc<Catalog>>,
    catalog_at: Option<i64>,
    preview: Option<Preview>,
    done: Vec<WriteDone>,
    jobs: Vec<Job>,
    episodes: Arc<Mutex<HashMap<String, Arc<Vec<Program>>>>>,
    suggestions: Arc<Vec<Suggestion>>,
    suggested_at: Option<i64>,
    surprise: Option<Suggestion>,
    dismissed: Option<dismissed::Map>,
    revision: u64,
}

impl Channels {
    pub fn channels(&self) -> &[VChannel] {
        &self.channels
    }

    pub fn channel(&self, playlist: &str) -> Option<&VChannel> {
        self.channels.iter().find(|c| c.playlist == playlist)
    }

    pub fn by_number(&self, number: &str) -> Option<&VChannel> {
        self.channels.iter().find(|c| c.number() == number)
    }

    pub fn list_state(&self) -> ListState {
        self.list
    }

    pub fn catalog(&self) -> Option<&Arc<Catalog>> {
        self.catalog.as_ref()
    }

    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }

    /// The suggested channels, best first ([`suggest::suggest`]).
    pub fn suggestions(&self) -> &Arc<Vec<Suggestion>> {
        &self.suggestions
    }

    pub fn suggestion(&self, id: &str) -> Option<&Suggestion> {
        self.suggestions.iter().find(|s| s.id == id).or(self.surprise.as_ref().filter(|s| s.id == id))
    }

    /// The last *Surprise me* draw.
    pub fn surprise(&self) -> Option<&Suggestion> {
        self.surprise.as_ref()
    }

    fn kept_origins(&self) -> Vec<String> {
        self.channels.iter().map(|c| c.recipe.origin.clone()).filter(|o| !o.is_empty()).collect()
    }

    fn dismissed_ids(&mut self) -> Vec<String> {
        let profile = self.scope.as_ref().map(|s| s.profile.clone()).unwrap_or_default();
        let map = self.dismissed.get_or_insert_with(dismissed::load);
        map.get(&profile).cloned().unwrap_or_default()
    }

    /// Writes that finished since the last call (the builder reads its own).
    pub fn take_done(&mut self) -> Vec<WriteDone> {
        std::mem::take(&mut self.done)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Drop everything when the server or profile changed. `true` when it did.
    fn rescope(&mut self) -> bool {
        let now = Scope::current();
        if self.scope.as_ref() == Some(&now) {
            return false;
        }
        *self = Channels { scope: Some(now), revision: self.revision + 1, ..Default::default() };
        true
    }

    pub fn run(&mut self, cmd: VCmd, now_ms: i64) -> bool {
        let rescoped = self.rescope();
        let changed = match cmd {
            VCmd::Refresh => self.start_list() | self.start_catalog(now_ms),
            VCmd::RefreshIfStale => {
                let stale = self.listed_at.map_or(true, |t| now_ms - t >= LIST_STALE_MS) || self.list == ListState::Failed;
                let mut c = false;
                if stale {
                    c |= self.start_list();
                }
                if self.catalog_at.map_or(true, |t| now_ms - t >= CATALOG_STALE_MS) {
                    c |= self.start_catalog(now_ms);
                }
                if self.suggested_at.map_or(true, |t| now_ms - t >= SUGGEST_STALE_MS) {
                    c |= self.start_suggest(now_ms);
                }
                c | self.rebuild_stale(now_ms)
            }
            VCmd::Suggest => self.start_suggest(now_ms),
            VCmd::Dismiss { id } => {
                let profile = self.scope.as_ref().map(|s| s.profile.clone()).unwrap_or_default();
                let map = self.dismissed.get_or_insert_with(dismissed::load);
                let added = dismissed::add(map, &profile, &id);
                if added {
                    dismissed::save(map);
                }
                let before = self.suggestions.len();
                self.suggestions = Arc::new(self.suggestions.iter().filter(|s| s.id != id).cloned().collect());
                if self.surprise.as_ref().is_some_and(|s| s.id == id) {
                    self.surprise = None;
                }
                added || before != self.suggestions.len()
            }
            VCmd::Surprise { seed } => self.start_surprise(now_ms, seed),
            VCmd::Preview { token, recipe } => self.start_preview(token, recipe),
            VCmd::ClearPreview => {
                self.jobs.retain(|j| !matches!(j, Job::Preview { .. }));
                self.preview.take().is_some()
            }
            VCmd::Keep { token, recipe } => self.start_keep(token, recipe),
            VCmd::Update { playlist, recipe, rebuild } => self.start_update(playlist, recipe, rebuild),
            VCmd::Delete { playlist } => self.start_delete(playlist),
        };
        if changed || rescoped {
            self.revision += 1;
        }
        changed || rescoped
    }

    fn client() -> Option<&'static plx_plex::plex::Client> {
        plx_plex::plex::client_for(plx_plex::plex::current_server())
    }

    fn spawn<T: Send + 'static>(name: &'static str, work: impl FnOnce() -> T + Send + 'static) -> Option<Receiver<T>> {
        if crate::stores::tape::active() {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = plx_base::task::spawn_small(name, move || {
            let _ = tx.send(work());
            plx_machine::idle::wake();
        });
        spawned.then_some(rx)
    }

    fn start_list(&mut self) -> bool {
        if self.jobs.iter().any(|j| matches!(j, Job::List(_))) {
            return false;
        }
        let Some(c) = Self::client() else { return false };
        let Some(rx) = Self::spawn("vchannel-list", move || remote::list(c)) else { return false };
        self.jobs.push(Job::List(rx));
        if self.list != ListState::Ready {
            self.list = ListState::Reading;
        }
        true
    }

    fn start_catalog(&mut self, now_ms: i64) -> bool {
        if self.jobs.iter().any(|j| matches!(j, Job::Catalog(_))) {
            return false;
        }
        let Some(c) = Self::client() else { return false };
        let Some(rx) = Self::spawn("vchannel-catalog", move || Catalog::fetch(c)) else { return false };
        self.jobs.push(Job::Catalog(rx));
        // Mark it asked now, so a failed read is retried on the stale clock, not every frame.
        self.catalog_at.get_or_insert(now_ms);
        true
    }

    fn build_job(&self, recipe: Recipe) -> Option<Receiver<Option<Vec<Program>>>> {
        let c = Self::client()?;
        let catalog = self.catalog.clone();
        let cache = Arc::clone(&self.episodes);
        Self::spawn("vchannel-build", move || {
            let mut reads = ClientReads { client: c, catalog: catalog.as_deref(), cache: &cache };
            programmes(&recipe, catalog.as_deref(), &mut reads)
        })
    }

    /// Start builds for channels never built or built before the catalog last landed, up to the
    /// in-flight bound. Library channels wait for the catalog.
    fn rebuild_stale(&mut self, now_ms: i64) -> bool {
        let mut started = false;
        let in_flight = |jobs: &Vec<Job>| jobs.iter().filter(|j| matches!(j, Job::Build { .. })).count();
        let wanted: Vec<(String, Recipe)> = self
            .channels
            .iter()
            .filter(|ch| {
                let needs_catalog = matches!(ch.recipe.source, super::recipe::Source::Library);
                let stale = ch.built_at_ms.map_or(true, |t| now_ms - t >= CATALOG_STALE_MS);
                stale && (!needs_catalog || self.catalog.is_some())
                    && !self.jobs.iter().any(|j| matches!(j, Job::Build { playlist, .. } if *playlist == ch.playlist))
            })
            .map(|ch| (ch.playlist.clone(), ch.recipe.clone()))
            .collect();
        for (playlist, recipe) in wanted {
            if in_flight(&self.jobs) >= BUILDS_IN_FLIGHT {
                break;
            }
            if let Some(rx) = self.build_job(recipe) {
                self.jobs.push(Job::Build { playlist, rx });
                started = true;
            }
        }
        started
    }

    fn start_suggest(&mut self, now_ms: i64) -> bool {
        let Some(cat) = self.catalog.clone() else { return false };
        if self.jobs.iter().any(|j| matches!(j, Job::Suggest(_))) {
            return false;
        }
        let kept = self.kept_origins();
        let dismissed = self.dismissed_ids();
        let Some(rx) = Self::spawn("vchannel-suggest", move || {
            suggest::suggest_excluding(&cat, &Moment::at(now_ms), &kept, &dismissed)
        }) else { return false };
        self.jobs.push(Job::Suggest(rx));
        self.suggested_at = Some(now_ms);
        true
    }

    fn start_surprise(&mut self, now_ms: i64, seed: u64) -> bool {
        let Some(cat) = self.catalog.clone() else { return false };
        self.jobs.retain(|j| !matches!(j, Job::Surprise(_)));
        let kept = self.kept_origins();
        let mut exclude = self.dismissed_ids();
        exclude.extend(self.surprise.iter().map(|s| s.id.clone()));
        let Some(rx) = Self::spawn("vchannel-surprise", move || {
            suggest::surprise(&cat, &Moment::at(now_ms), &kept, &exclude, seed)
        }) else { return false };
        self.jobs.push(Job::Surprise(rx));
        true
    }

    fn start_preview(&mut self, token: u64, recipe: Recipe) -> bool {
        self.jobs.retain(|j| !matches!(j, Job::Preview { .. }));
        // Reshuffle and a new style change only the seed or style: reuse the programmes.
        if let Some(p) = self.preview.as_mut() {
            if let Some(s) = p.schedule.as_ref().filter(|_| p.recipe.source == recipe.source && p.recipe.rules == recipe.rules) {
                let s = Schedule::new(s.programs().to_vec(), recipe.style, recipe.seed, recipe.epoch_ms);
                *p = Preview { token, recipe, schedule: Some(s), failed: false };
                return true;
            }
        }
        let Some(rx) = self.build_job(recipe.clone()) else {
            self.preview = Some(Preview { token, recipe, schedule: None, failed: true });
            return true;
        };
        self.jobs.push(Job::Preview { token, rx });
        self.preview = Some(Preview { token, recipe, schedule: None, failed: false });
        true
    }

    fn start_keep(&mut self, token: u64, mut recipe: Recipe) -> bool {
        let Some(c) = Self::client() else {
            self.done.push(WriteDone::Failed { token });
            return true;
        };
        recipe.number = next_number(self.channels.iter().map(|ch| ch.recipe.number));
        // The programmes the preview built ride along as the playlist's first items, and seed the
        // new channel's timeline so it airs at once.
        let programmes: Vec<Program> = self
            .preview
            .as_ref()
            .filter(|p| p.token == token)
            .and_then(|p| p.schedule.as_ref())
            .map(|s| s.programs().iter().take(PROGRAMMES_MAX).cloned().collect())
            .unwrap_or_default();
        let order: Vec<Program> = self
            .preview
            .as_ref()
            .and_then(|p| p.schedule.as_ref())
            .map(|s| s.pass_order(0).iter().map(|&i| s.programs()[i].clone()).collect())
            .unwrap_or_default();
        let kept = recipe.clone();
        let catalog = self.catalog.clone();
        let cache = Arc::clone(&self.episodes);
        let preview_built = !programmes.is_empty();
        let held = Arc::clone(&self.episodes);
        let Some(rx) = Self::spawn("vchannel-keep", move || {
            // Kept straight from a suggestion's tile, with no preview: build it here first, so
            // the playlist has items and the channel airs the moment it lands.
            let order = if order.is_empty() {
                let mut reads = ClientReads { client: c, catalog: catalog.as_deref(), cache: &cache };
                let built = super::build::programmes(&kept, catalog.as_deref(), &mut reads).unwrap_or_default();
                if built.is_empty() {
                    return (WriteDone::Failed { token }, None);
                }
                let s = Schedule::new(built.clone(), kept.style, kept.seed, kept.epoch_ms);
                if !preview_built {
                    if let Ok(mut m) = held.lock() {
                        m.insert(format!("keep:{token}"), Arc::new(built));
                    }
                }
                s.pass_order(0).iter().map(|&i| s.programs()[i].clone()).collect()
            } else {
                order
            };
            match remote::keep(c, &kept, &order) {
                Some(playlist) => (
                    WriteDone::Kept { token, playlist: playlist.clone() },
                    Some(Stored { playlist, recipe: kept, composite: String::new() }),
                ),
                None => (WriteDone::Failed { token }, None),
            }
        }) else {
            self.done.push(WriteDone::Failed { token });
            return true;
        };
        self.jobs.push(Job::Write(rx));
        // Hold the built programmes for the landing.
        if !programmes.is_empty() {
            if let Ok(mut m) = self.episodes.lock() {
                m.insert(format!("keep:{token}"), Arc::new(programmes));
            }
        }
        true
    }

    fn start_update(&mut self, playlist: String, recipe: Recipe, rebuild: bool) -> bool {
        let Some(c) = Self::client() else { return false };
        // The change shows at once; the write follows.
        let mut programmes: Option<Vec<Program>> = None;
        if let Some(ch) = self.channels.iter_mut().find(|ch| ch.playlist == playlist) {
            let same_source = ch.recipe.source == recipe.source && ch.recipe.rules == recipe.rules;
            if let Some(s) = ch.schedule.as_ref().filter(|_| same_source) {
                ch.schedule = Some(Schedule::new(s.programs().to_vec(), recipe.style, recipe.seed, recipe.epoch_ms));
                programmes = Some(ch.schedule.as_ref().map(|s| s.pass_order(0).iter().map(|&i| s.programs()[i].clone()).collect()).unwrap_or_default());
            }
            if !same_source || rebuild {
                ch.built_at_ms = None;
            }
            ch.recipe = recipe.clone();
        }
        let p = playlist.clone();
        if let Some(rx) = Self::spawn("vchannel-update", move || {
            let ok = remote::update(c, &p, &recipe, programmes.as_deref());
            (if ok { WriteDone::Updated { playlist: p } } else { WriteDone::Failed { token: 0 } }, None)
        }) {
            self.jobs.push(Job::Write(rx));
        }
        true
    }

    fn start_delete(&mut self, playlist: String) -> bool {
        let Some(c) = Self::client() else { return false };
        let before = self.channels.len();
        self.channels.retain(|ch| ch.playlist != playlist);
        let p = playlist.clone();
        if let Some(rx) = Self::spawn("vchannel-delete", move || {
            let ok = remote::delete(c, &p);
            (if ok { WriteDone::Deleted { playlist: p } } else { WriteDone::Failed { token: 0 } }, None)
        }) {
            self.jobs.push(Job::Write(rx));
        }
        before != self.channels.len()
    }

    /// Drain the workers. `true` when anything changed.
    pub fn pump(&mut self, now_ms: i64) -> bool {
        if self.jobs.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut landed_catalog = false;
        let jobs = std::mem::take(&mut self.jobs);
        for job in jobs {
            match job {
                Job::List(rx) => match rx.try_recv() {
                    Ok(Some(stored)) => {
                        self.adopt(stored);
                        self.list = ListState::Ready;
                        self.listed_at = Some(now_ms);
                        changed = true;
                    }
                    Ok(None) | Err(TryRecvError::Disconnected) => {
                        if self.list != ListState::Ready {
                            self.list = ListState::Failed;
                        }
                        self.listed_at = Some(now_ms);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => self.jobs.push(Job::List(rx)),
                },
                Job::Catalog(rx) => match rx.try_recv() {
                    Ok(cat) => {
                        if let Some(cat) = cat {
                            plx_base::eventlog::log(&format!("vchannel: catalog films={} shows={}", cat.movies.len(), cat.shows.len()));
                            self.catalog = Some(Arc::new(cat));
                            if let Ok(mut m) = self.episodes.lock() {
                                m.clear();
                            }
                            landed_catalog = true;
                        }
                        self.catalog_at = Some(now_ms);
                        changed = true;
                    }
                    Err(TryRecvError::Disconnected) => changed = true,
                    Err(TryRecvError::Empty) => self.jobs.push(Job::Catalog(rx)),
                },
                Job::Build { playlist, rx } => match rx.try_recv() {
                    Ok(result) => {
                        if let Some(ch) = self.channels.iter_mut().find(|c| c.playlist == playlist) {
                            match result {
                                Some(p) if !p.is_empty() => {
                                    ch.schedule = Some(Schedule::new(p, ch.recipe.style, ch.recipe.seed, ch.recipe.epoch_ms));
                                    ch.failed = false;
                                }
                                _ => ch.failed = true,
                            }
                            ch.built_at_ms = Some(now_ms);
                        }
                        changed = true;
                    }
                    Err(TryRecvError::Disconnected) => changed = true,
                    Err(TryRecvError::Empty) => self.jobs.push(Job::Build { playlist, rx }),
                },
                Job::Preview { token, rx } => match rx.try_recv() {
                    Ok(result) => {
                        if let Some(p) = self.preview.as_mut().filter(|p| p.token == token) {
                            match result {
                                Some(list) if !list.is_empty() => {
                                    p.schedule = Some(Schedule::new(list, p.recipe.style, p.recipe.seed, p.recipe.epoch_ms));
                                }
                                _ => p.failed = true,
                            }
                        }
                        changed = true;
                    }
                    Err(TryRecvError::Disconnected) => changed = true,
                    Err(TryRecvError::Empty) => self.jobs.push(Job::Preview { token, rx }),
                },
                Job::Suggest(rx) => match rx.try_recv() {
                    Ok(list) => {
                        plx_base::eventlog::log(&format!("vchannel: suggestions n={}", list.len()));
                        self.suggestions = Arc::new(list);
                        changed = true;
                    }
                    Err(TryRecvError::Disconnected) => changed = true,
                    Err(TryRecvError::Empty) => self.jobs.push(Job::Suggest(rx)),
                },
                Job::Surprise(rx) => match rx.try_recv() {
                    Ok(pick) => {
                        self.surprise = pick;
                        changed = true;
                    }
                    Err(TryRecvError::Disconnected) => changed = true,
                    Err(TryRecvError::Empty) => self.jobs.push(Job::Surprise(rx)),
                },
                Job::Write(rx) => match rx.try_recv() {
                    Ok((done, stored)) => {
                        if let (WriteDone::Kept { token, .. }, Some(stored)) = (&done, stored) {
                            let built = self.episodes.lock().ok().and_then(|mut m| m.remove(&format!("keep:{token}")));
                            let schedule = built.map(|p| Schedule::new(p.as_ref().clone(), stored.recipe.style, stored.recipe.seed, stored.recipe.epoch_ms));
                            self.channels.push(VChannel {
                                playlist: stored.playlist,
                                recipe: stored.recipe,
                                composite: stored.composite,
                                built_at_ms: schedule.as_ref().map(|_| now_ms),
                                schedule,
                                failed: false,
                            });
                            self.channels.sort_by_key(|c| c.recipe.number);
                        }
                        self.done.push(done);
                        changed = true;
                    }
                    Err(TryRecvError::Disconnected) => changed = true,
                    Err(TryRecvError::Empty) => self.jobs.push(Job::Write(rx)),
                },
            }
        }
        if landed_catalog {
            changed |= self.start_suggest(now_ms);
        }
        if landed_catalog || changed {
            changed |= self.rebuild_stale(now_ms);
        }
        // A kept suggestion leaves the row at once.
        let kept = self.kept_origins();
        if self.suggestions.iter().any(|s| kept.contains(&s.id)) {
            self.suggestions = Arc::new(self.suggestions.iter().filter(|s| !kept.contains(&s.id)).cloned().collect());
            changed = true;
        }
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// Take a fresh list from the server, keeping the timelines of channels whose recipe did not
    /// change.
    fn adopt(&mut self, stored: Vec<Stored>) {
        let old = std::mem::take(&mut self.channels);
        for s in stored {
            let kept = old.iter().find(|c| c.playlist == s.playlist);
            let (schedule, built_at_ms) = match kept {
                Some(c) if c.recipe.source == s.recipe.source && c.recipe.rules == s.recipe.rules => {
                    let schedule = c.schedule.as_ref().map(|sch| {
                        if sch.style() == s.recipe.style && sch.seed() == s.recipe.seed && sch.epoch_ms() == s.recipe.epoch_ms {
                            sch.clone()
                        } else {
                            Schedule::new(sch.programs().to_vec(), s.recipe.style, s.recipe.seed, s.recipe.epoch_ms)
                        }
                    });
                    (schedule, c.built_at_ms)
                }
                _ => (None, None),
            };
            self.channels.push(VChannel { playlist: s.playlist, recipe: s.recipe, composite: s.composite, schedule, built_at_ms, failed: false });
        }
    }

    /// Install channels directly (tests and the simulator's fixtures).
    #[cfg(any(test, feature = "test-support"))]
    pub fn install_for_test(&mut self, channels: Vec<VChannel>) {
        self.scope = Some(Scope::current());
        self.channels = channels;
        self.list = ListState::Ready;
        self.revision += 1;
    }
}
