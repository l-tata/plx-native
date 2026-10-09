//! **Live TV's half of the loop**: what the Live TV page and the player's live banner ask for
//! (`LiveTvReq`), performed with the playback session, the adapter and the navigation a screen may
//! not name (§2.1).
//!
//! A TUNE is three steps, the middle one on a worker:
//!
//! 1. Stop whatever is playing (a film, or the previous channel — closing its connection first, so
//!    Tunarr can hand the transcode to the next viewer or keep it warm for a quick zap back), mark
//!    the session as this channel TUNING (`route::set_live_tuning`), and put the player on screen
//!    with the banner saying so.
//! 2. Probe the channel's stream (`plx_media::live::probe`): its codecs and frame rate, which the
//!    Load payload must declare.
//! 3. When the probe lands — and only if the session is still on that channel — install the
//!    channel as the playback (`route::install_live_stream`) and start the engine. A failed probe
//!    leaves the banner saying the channel is unavailable, and OK retries.
//!
//! A live stream that ENDS (Tunarr restarted, the network dropped) is re-tuned rather than left,
//! a bounded number of times; an engine failure is retried the same way before the failure
//! read-out is allowed to stand.

use super::bridge::{self, AppHost};
use super::App;
use plx_machine::machine::{InputOwner, MachineId};
use plx_media::live::{LiveSession, ProbeError, StreamFacts};
use plx_screens::registry::{AppArg, HomeTab, LiveTvReq};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

/// How many times a dropped or failed channel is re-tuned on its own before the viewer is asked.
const AUTO_RETRIES: u32 = 3;
/// The wait before each automatic re-tune, in frame-clock ms.
const RETRY_DELAY_MS: u32 = 2_000;

struct Pending {
    /// Which tune this probe belongs to; a newer tune supersedes it.
    epoch: u64,
    url: String,
    rx: Receiver<Result<StreamFacts, ProbeError>>,
}

/// The tuner: the probe in flight, and the automatic re-tune schedule.
#[derive(Default)]
pub(crate) struct LiveTuner {
    epoch: u64,
    pending: Option<Pending>,
    /// Automatic re-tunes spent on the current channel since it last played.
    retries: u32,
    /// When to re-tune on our own (frame-clock ms), if anything is owed.
    retry_at: Option<u32>,
}

/// Drain the Live TV requests the screens raised this frame.
pub(crate) fn requests(app: &mut App, now: u32) {
    for (source, request, ret) in app.bridge.take_livetv_reqs() {
        // Settings > Live TV comes from a page inside the Settings surface, whose inner stack is
        // not the container's entry; it only navigates, so it needs no owner check.
        if request == LiveTvReq::OpenSetup {
            bridge::dismiss_settings(&mut app.pages);
            open_page(&mut app.pages, &mut app.bridge, true);
            continue;
        }
        let MachineId::Instance(instance) = source else { continue };
        let Some(entry) = app.pages.nav.entry_of_instance(instance) else { continue };
        if app.pages.nav.input_owner() != Some(InputOwner::Entry(entry)) { continue; }
        match request {
            LiveTvReq::Tab(tab) => {
                if !app.bridge.search_tab_available(tab) || matches!(tab, HomeTab::LiveTv) { continue; }
                match tab {
                    HomeTab::Home => bridge::nav_home_pill(&mut app.pages, &mut app.bridge, Some(ret)),
                    other => bridge::nav_tab(&mut app.pages, &mut app.bridge, other, None, Some(ret)),
                }
            }
            LiveTvReq::Account => bridge::open_account_menu(&mut app.pages),
            LiveTvReq::Back => bridge::nav_tab(&mut app.pages, &mut app.bridge, HomeTab::Home, None, Some(ret)),
            LiveTvReq::Tune { index } => {
                let lineup = Arc::clone(app.bridge.livetv_view().lineup());
                if index < lineup.len() {
                    let previous = plx_media::route::live(&app.player.session).map(|l| l.index).filter(|&p| p != index);
                    tune(app, lineup, index, previous, now);
                }
            }
            LiveTvReq::Step(delta) => {
                let Some(live) = plx_media::route::live(&app.player.session).cloned() else { continue };
                if let Some(next) = live.lineup.step(live.index, delta) {
                    tune(app, Arc::clone(&live.lineup), next, Some(live.index), now);
                }
            }
            LiveTvReq::Previous => {
                let Some(live) = plx_media::route::live(&app.player.session).cloned() else { continue };
                if let Some(prev) = live.previous.filter(|&p| p < live.lineup.len()) {
                    tune(app, Arc::clone(&live.lineup), prev, Some(live.index), now);
                }
            }
            LiveTvReq::Typed(number) => {
                let Some(live) = plx_media::route::live(&app.player.session).cloned() else { continue };
                if let Some(index) = live.lineup.index_for_typed(&number) {
                    if index != live.index {
                        tune(app, Arc::clone(&live.lineup), index, Some(live.index), now);
                    }
                }
            }
            LiveTvReq::Detail { sid, rk } => {
                let hit = plx_data::livetv::plexmatch::Hit { sid, rk };
                if !app.bridge.livetv_view().knows_match(&hit) { continue; }
                let arg = AppArg::Content(plx_screens::registry::ContentArg::Detail { sid: hit.sid, rk: hit.rk });
                bridge::nav_push_with_return(&mut app.pages, arg, ret);
            }
            LiveTvReq::Retry => retry(app, now),
            LiveTvReq::OpenSetup => {}
        }
    }
}

/// Tune the channel numbered `number` in the loaded guide — OK on Home's On Now card
/// (`HomeReq::Tune`). A number the guide no longer lists (it reloaded under the card) does nothing.
pub(crate) fn tune_number(app: &mut App, number: &str, now: u32) {
    let lineup = Arc::clone(app.bridge.livetv_view().lineup());
    let Some(index) = lineup.index_of_number(number) else { return };
    let previous = plx_media::route::live(&app.player.session).map(|l| l.index).filter(|&p| p != index);
    tune(app, lineup, index, previous, now);
}

/// Tune the playing channel again — the failure read-out's *Try again* and the banner's OK on a
/// failed tune.
pub(crate) fn retry(app: &mut App, now: u32) {
    let Some(live) = plx_media::route::live(&app.player.session).cloned() else { return };
    app.livetv.retries = 0;
    tune(app, live.lineup, live.index, live.previous, now);
}

fn tune(app: &mut App, lineup: Arc<plx_data::livetv::guide::Lineup>, index: usize, previous: Option<usize>, now: u32) {
    let Some(channel) = lineup.channels.get(index) else { return };
    let url = channel.url.clone();
    plx_base::eventlog::log(&format!("livetv: tune channel={} index={index}", channel.number));
    // Step 1: stop what plays, and show the channel tuning.
    super::content::halt_preview_now(&mut app.player.session, &mut app.adapters.player);
    plx_media::route::cancel_play(&mut app.player.session);
    super::playback::close_player_overlays(&mut app.pages);
    plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    plx_media::player::report::clear_error_trace();
    let session = LiveSession::tuning(lineup, index, previous);
    plx_media::route::set_live_tuning(&mut app.player.session, session);
    let in_player = app.route() == AppArg::Player;
    if in_player {
        if let Some(player) = bridge::player_mut(&mut app.pages) {
            player.hud.dismissed = false;
            player.hud.extend(now, plx_screens::player::LIVE_BANNER_MS);
            player.publish();
        }
    } else {
        super::playback::enter_player(&mut app.pages, &mut app.bridge, super::playback::Origin::Here, None);
        app.bridge.seed_player_hud(plx_screens::player::LIVE_BANNER_MS);
    }
    plx_media::player::lifecycle::set_paused(false);
    // Step 2: probe on a worker.
    app.livetv.epoch += 1;
    app.livetv.retry_at = None;
    let (tx, rx) = mpsc::channel();
    let probe_url = url.clone();
    let spawned = plx_base::task::spawn_small("livetv-probe", move || {
        let _ = tx.send(plx_media::live::probe(&probe_url));
        plx_machine::idle::wake();
    });
    if spawned {
        app.livetv.pending = Some(Pending { epoch: app.livetv.epoch, url, rx });
    } else {
        // No worker: play with Tunarr's default declaration rather than not at all.
        start(app, url, StreamFacts::TUNARR_DEFAULT);
    }
    plx_machine::idle::invalidate();
}

/// Step 3: install the channel and start the engine.
fn start(app: &mut App, url: String, facts: StreamFacts) {
    let Some(mut session) = plx_media::route::live(&app.player.session).cloned() else { return };
    session.facts = Some(facts);
    session.failed = false;
    plx_base::eventlog::log(&format!(
        "livetv: start vcodec={} acodec={} fps={:.3}",
        facts.vcodec, facts.acodec, facts.fps
    ));
    if !plx_media::route::install_live_stream(&mut app.player.session, session, &url) {
        fail(app);
        return;
    }
    if !plx_media::player::start_bufferfeed(&mut app.player.session, &mut app.adapters.player) {
        plx_base::eventlog::log("livetv: the engine refused to start");
        fail(app);
        return;
    }
    plx_media::player::lifecycle::set_paused(false);
    if let Some(channel) = plx_media::route::live(&app.player.session).and_then(|l| l.channel()) {
        plx_data::livetv::remember_channel(&channel.number);
    }
    plx_machine::idle::invalidate();
}

fn fail(app: &mut App) {
    if let Some(mut session) = plx_media::route::live(&app.player.session).cloned() {
        session.failed = true;
        plx_media::route::set_live_tuning(&mut app.player.session, session);
    }
    plx_machine::idle::invalidate();
}

/// Once a frame: land the probe, and run the automatic re-tune schedule.
pub(crate) fn pump(app: &mut App, now: u32) {
    if let Some(pending) = app.livetv.pending.as_ref() {
        match pending.rx.try_recv() {
            Ok(result) => {
                let Pending { epoch, url, .. } = app.livetv.pending.take().expect("checked above");
                let current = epoch == app.livetv.epoch && plx_media::route::live(&app.player.session).is_some();
                if current {
                    match result {
                        Ok(facts) => start(app, url, facts),
                        // A stream that answered but could not be read — the probe found no PMT in
                        // the bytes it got, say — may still play: try with the default declaration.
                        Err(ProbeError::NotPlayable(why)) => {
                            plx_base::eventlog::log(&format!("livetv: probe inconclusive ({why}); playing with the default declaration"));
                            start(app, url, StreamFacts::TUNARR_DEFAULT);
                        }
                        Err(e) => {
                            plx_base::eventlog::log(&format!("livetv: probe failed ({e})"));
                            fail(app);
                            schedule_retry(app, now);
                        }
                    }
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                app.livetv.pending = None;
                fail(app);
            }
        }
    }
    let Some(live) = plx_media::route::live(&app.player.session).cloned() else {
        app.livetv.retry_at = None;
        return;
    };
    // A channel that has shown a picture has proven itself: its retry budget is whole again.
    if live.facts.is_some() && plx_media::player::seen_frame() {
        app.livetv.retries = 0;
    }
    // An engine that failed on a live channel is re-tuned before the read-out is left standing.
    if app.livetv.retry_at.is_none()
        && live.facts.is_some()
        && plx_appkit::player_hud::transport_hidden(&app.player.session)
    {
        schedule_retry(app, now);
    }
    if app.livetv.retry_at.is_some_and(|at| now.wrapping_sub(at) < u32::MAX / 2) {
        app.livetv.retry_at = None;
        if app.route() == AppArg::Player {
            plx_base::eventlog::log(&format!("livetv: automatic re-tune {}/{}", app.livetv.retries, AUTO_RETRIES));
            tune(app, live.lineup, live.index, live.previous, now);
        }
    }
}

fn schedule_retry(app: &mut App, now: u32) {
    if app.livetv.retries < AUTO_RETRIES {
        app.livetv.retries += 1;
        app.livetv.retry_at = Some(now.wrapping_add(RETRY_DELAY_MS));
    }
}

/// The live stream drained to its end: a channel does not end, so this is a dropped connection.
/// Re-tune it (bounded); when the budget is spent, leave the banner saying it is unavailable.
pub(crate) fn stream_ended(app: &mut App, now: u32) {
    plx_base::eventlog::log("livetv: the stream ended; re-tuning");
    plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    if app.livetv.retries < AUTO_RETRIES {
        schedule_retry(app, now);
    } else {
        fail(app);
    }
}

/// Open the Live TV page on its channel studio, on the suggestion `id` (a Home Suggested Channels
/// card).
pub(crate) fn open_studio(app: &mut App, id: String) {
    bridge::nav_select_tab(&mut app.pages, AppArg::LiveTv);
    app.bridge.request_livetv_studio(id);
}

/// Open the Live TV page — on its setup face when asked (Settings > Live TV).
pub(crate) fn open_page(d: &mut plx_ui::dispatch::Dispatcher<AppHost>, rig: &mut bridge::Bridge, setup: bool) {
    bridge::nav_select_tab(d, AppArg::LiveTv);
    if setup {
        rig.request_livetv_setup();
    }
}
