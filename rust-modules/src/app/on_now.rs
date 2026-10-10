//! **Home's On Now shelf, fed** — the Live TV store's guide turned into Home's live row
//! (`plx_data::livetv::on_now`) and handed to the hub store (`HubsCmd::SetOnNow`), which places it
//! after the watchlist.
//!
//! The rows are rebuilt when anything they draw can have moved: the guide (a load landed, the
//! server changed or was turned off), the wall-clock MINUTE (a programme ended, a progress bar
//! moved), the channel last tuned (it leads the row), or the server whose photo transcoder fetches
//! the artwork. Nothing else — the hub store also refuses an identical shelf, so the minute tick
//! re-publishes Home only when a card really changed.
//!
//! Home is where the shelf is seen, so Home showing is also where a stale guide is reloaded
//! (`LiveTvCmd::RefreshIfStale`, the Live TV page's own rule): on each arrival of Home, never on
//! a timer behind a page nobody is looking at.
//!
//! OK on a card is `HomeReq::Tune`, performed by `livetv::tune_number`.
//!
//! Home's **Suggested Channels** shelf rides the same feed: the channels the library could air
//! for this profile (`plx_data::livetv::suggested`), handed over (`HubsCmd::SetChannels`) whenever
//! the virtual channels' state moved. Home arriving also asks for them to be read and worked out
//! again when stale (`VCmd::RefreshIfStale`) — with or without a Tunarr server, since they come
//! from the Plex library. OK on one is `HomeReq::Studio`.

use super::App;
use plx_data::livetv::on_now;
use plx_screens::registry::AppArg;

/// Everything the shelf's rows are a function of, besides the guide's own contents.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Inputs {
    /// `LiveTvView::revision` — moves with every guide landing and every server change.
    pub(crate) guide: u64,
    /// The wall-clock minute.
    pub(crate) minute: i64,
    /// `Session::livetv_channel`.
    pub(crate) last: String,
    /// The server the artwork is fetched through.
    pub(crate) art_sid: u16,
}

/// What the feed remembers between frames.
#[derive(Default)]
pub(crate) struct OnNowFeed {
    /// The inputs of the rows last handed over; `None` before the first.
    built: Option<Inputs>,
    /// Was Home the page on screen last frame? Its rising edge reloads a stale guide.
    home_visible: bool,
    /// The virtual channels' revision the Suggested Channels shelf was last built from.
    channels_built: Option<u64>,
}

impl OnNowFeed {
    /// One frame's decisions, pure: `(reload the guide, rebuild the rows)`.
    pub(crate) fn step(&mut self, inputs: Inputs, configured: bool, home_visible: bool) -> (bool, bool) {
        let arrived = home_visible && !self.home_visible;
        self.home_visible = home_visible;
        let rebuild = self.built.as_ref() != Some(&inputs);
        if rebuild {
            self.built = Some(inputs);
        }
        (arrived && configured, rebuild)
    }
}

/// Once a frame from the loop.
pub(crate) fn step(app: &mut App) {
    let now_ms = plx_base::wallclock::now_ms();
    let art_sid = plx_plex::plex::current_server();
    let view = app.bridge.livetv_view();
    let configured = view.configured();
    let inputs = Inputs {
        guide: view.revision(),
        minute: now_ms.div_euclid(60_000),
        last: plx_plex::plex::session::peek().livetv_channel().to_owned(),
        art_sid: art_sid.raw(),
    };
    let home_visible = matches!(app.route(), AppArg::Home);
    let tunarr = view.tunarr_configured();
    let arrived = home_visible && !app.on_now.home_visible;
    let (reload, rebuild) = app.on_now.step(inputs, configured, home_visible);
    if reload && tunarr {
        app.bridge.livetv_run(plx_data::livetv::LiveTvCmd::RefreshIfStale);
    }
    // On each arrival of Home, and while Home shows and the channels were never read (the Plex
    // server was not ready yet when Home first appeared): a read with no server does nothing.
    let unread = {
        let v = app.bridge.livetv_view();
        v.virtuals().list_state() == plx_data::vchannel::channels::ListState::Unread && !v.virtuals().busy()
    };
    if arrived || (home_visible && unread) {
        app.bridge.livetv_run(plx_data::livetv::LiveTvCmd::Virtual(plx_data::vchannel::channels::VCmd::RefreshIfStale));
    }
    feed_channels(app);
    if !rebuild {
        return;
    }
    let view = app.bridge.livetv_view();
    let rows = if view.configured() {
        let last = plx_plex::plex::session::peek().livetv_channel().to_owned();
        on_now::rows(&on_now::on_now(view.lineup(), &last, now_ms, on_now::ON_NOW_MAX), art_sid, now_ms)
    } else {
        Vec::new()
    };
    super::bridge::execute_endpoint_outcomes(
        &mut app.pages,
        app.bridge.hubs_run(plx_data::stores::hubs::HubsCmd::SetOnNow(plx_data::pms::ShelfRows(rows))).endpoints,
    );
}

/// Hand Home the Suggested Channels shelf when the virtual channels moved since it was built.
fn feed_channels(app: &mut App) {
    let view = app.bridge.livetv_view();
    let revision = view.virtuals().revision();
    if app.on_now.channels_built == Some(revision) {
        return;
    }
    app.on_now.channels_built = Some(revision);
    let rows = plx_data::livetv::suggested::rows(
        view.virtuals().suggestions(),
        plx_platform::i18n::msg::browse_home_surprise_title(),
        plx_platform::i18n::msg::browse_home_surprise_why(),
    );
    super::bridge::execute_endpoint_outcomes(
        &mut app.pages,
        app.bridge.hubs_run(plx_data::stores::hubs::HubsCmd::SetChannels(plx_data::pms::ShelfRows(rows))).endpoints,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(guide: u64, minute: i64) -> Inputs {
        Inputs { guide, minute, last: String::new(), art_sid: 0 }
    }

    #[test]
    fn the_rows_are_rebuilt_only_when_something_they_draw_moved() {
        let mut feed = OnNowFeed::default();
        assert_eq!(feed.step(at(1, 10), true, false), (false, true), "the first frame builds");
        assert_eq!(feed.step(at(1, 10), true, false), (false, false), "nothing moved");
        assert_eq!(feed.step(at(1, 11), true, false), (false, true), "the minute turned");
        assert_eq!(feed.step(at(2, 11), true, false), (false, true), "the guide landed");
        let mut tuned = at(2, 11);
        tuned.last = "7".into();
        assert_eq!(feed.step(tuned, true, false), (false, true), "a channel was tuned: it leads now");
    }

    #[test]
    fn home_arriving_reloads_a_configured_guide_once() {
        let mut feed = OnNowFeed::default();
        assert!(feed.step(at(1, 1), true, true).0, "Home shown");
        assert!(!feed.step(at(1, 1), true, true).0, "still shown: no second reload");
        assert!(!feed.step(at(1, 1), true, false).0, "another page");
        assert!(feed.step(at(1, 1), true, true).0, "Home again");
        let mut off = OnNowFeed::default();
        assert!(!off.step(at(1, 1), false, true).0, "no Live TV server: nothing to reload");
    }
}
