//! **The Live TV page** — the guide of the configured Tunarr server's channels and the profile's
//! virtual channels (`plx_data::vchannel`), the channel studio where virtual channels are suggested
//! and kept, or, when there is neither Tunarr nor anything from the library, the setup that finds
//! a Tunarr server. A peer of Home, the Library and Search on the
//! top strip (`AppArg::LiveTv`). The data is `plx_data::livetv` (the store's view through
//! [`LiveTvLike`]); tuning a channel is a request the loop performs (`LiveTvReq::Tune`), because it
//! needs the playback session and the adapter (§2.1).
//!
//! Four faces, chosen from the store every frame ([`Face::page`]). Without a Tunarr server (or with
//! one that failed) the guide still shows the channels the profile kept, and the **Studio** — the
//! channel studio ([`studio`]) — stands in for the setup when the library can suggest channels; it
//! is also opened from the guide's Channels pill and from a Home Suggested Channels card.
//!
//! * **Setup** — no server, a server that failed to load, or the viewer asked to change it: an
//!   explanation, *Search the network* (SSDP, `LiveTvCmd::Discover`), one row per Tunarr found, an
//!   address field on the television's own keyboard, and — when a server is configured — *Try
//!   again* and *Turn off Live TV*.
//! * **Loading** — the first load of a configured server.
//! * **Guide** — an info pane for the airing under the cursor (its artwork, title, episode, times,
//!   how long is left, category and description, over a ground keyed from the artwork), a genre
//!   strip ([`filter`]), and a grid of channel rows — each led by the channel's logo tile
//!   (`plx_ui::channel_tile`) — against a two-hour time axis with a NOW marker. The cursor is a
//!   moment ([`grid::Cursor`]); OK tunes the focused channel, digits jump to a channel number,
//!   CH▲/▼ page. When the focused airing is a film or an episode in the viewer's Plex library
//!   (`plx_data::livetv::plexmatch`, looked up once focus has rested on it for [`MATCH_DWELL_MS`]),
//!   the pane says so and a HELD OK opens it there (`LiveTvReq::Detail`) to watch from the start.
//!
//! **Focus.** The page declares ONE focusable element to the engine ([`ELEM`]) and keeps its own
//! cursor inside it — the guide's moment-and-row or its genre strip, or the setup's row — the way
//! the person page's biography keeps its page (`person_bio.rs`). An UP the cursor cannot take (the
//! genre strip, the first setup row) is returned to the engine, whose `Link` carries it to the
//! top strip; the top strip's DOWN comes back the same way. On the guide the element is a CARD to
//! the engine, so OK arms a press that can be held: a release tunes, a hold opens the Plex match.

mod draw;
pub mod filter;
pub mod grid;
pub mod studio;
mod studio_draw;

use std::borrow::Cow;

use crate::registry::{AppFx, HomeTab, LiveTvLike, LiveTvReq, PageMemory};
use filter::Filter;
use grid::{Cursor, Rows};
use plx_data::livetv::guide::{Airing, Channel, Lineup};
use plx_data::livetv::plexmatch::{Hit, Want};
use plx_data::livetv::{Discovery, LiveTvCmd, LiveTvView, PlexMatch, Status};
use plx_data::stores::{StoreCmd, StoreId};
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind,
    InstanceId, Key, LogicalState, Machine, MachineId, TextEdit,
};
use plx_machine::present::Provenance;
use plx_ui::consts::{MARGIN_X, MARGIN_Y, SCR_H, SCR_W};
use plx_ui::frame::Budget;
use plx_ui::screen::{
    At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Enter, FocusTarget, Focusable, GroupKind,
    GroupSpec, Link, Placed, RenderStrategy, Screen, ScreenEvent, Seat, Step,
};
use plx_ui::text_buffer::TextBuffer;
use plx_ui::widgets::{StatusKind, StatusOverlay};
use plx_ui::{theme, Rect};

/// The page's one engine element, and its group.
pub const ELEM: u32 = 1;
const GROUP: GroupId = GroupId(0x4C54_5600);
const STRIP: GroupId = plx_ui::containers::tabs::STRIP;
/// The strip pill this page answers to (`app::chrome`'s Live TV key).
pub const STRIP_LIVETV_ELEM: u32 = plx_ui::dispatch::STRIP_BASE + 5;

/// How long a typed channel number waits for its next digit.
const DIGIT_MS: u32 = 1_500;

/// How long a guide airing must hold the cursor before it is looked up in the viewer's Plex
/// library: long enough that walking across the grid asks nothing, short enough that the hint is
/// there by the time the viewer has read the title.
pub const MATCH_DWELL_MS: u32 = 600;

/// The fields [`LiveTvScreen`] canonicalises, for the recorder's shape pin (§5.4).
pub const SHAPE: &str = "LiveTvScreen{entry:u32,instance:u32,cursor:{row:u64,at:i64,window:i64,top:u64,follow:bool},setup_sel:u64,force_setup:bool,editing:bool,address:str,typed:str,searched:bool,remembered:str,seated:bool,filter:str,on_strip:bool,strip_sel:u64,studio:str}";

// ---- geometry ----------------------------------------------------------------------------------

const TOP: f32 = plx_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
/// The guide's info pane: its text column on the left, the artwork on the right edge.
const INFO_H: f32 = 216.0;
const INFO_W: f32 = 1_200.0;
/// The artwork's box: as tall as a 16:9 picture of [`ART_MAX_W`]; a poster stands narrower in it.
const ART_H: f32 = 180.0;
const ART_MAX_W: f32 = ART_H * 16.0 / 9.0;
/// The genre strip.
const STRIP_TOP: f32 = TOP + INFO_H + theme::space::MD;
const STRIP_H: f32 = 48.0;
const CHIP_GAP: f32 = theme::space::SM;
/// The longest a guide category's chip may be before its label is cut (a genre's own word always
/// fits: `livetv_chip_labels_fit` holds it).
const CHIP_LABEL_MAX: f32 = 260.0;
/// The time axis band and the rows under it. The axis is drawn once, above the rows, and never
/// scrolls: paging moves the rows under it.
const AXIS_TOP: f32 = STRIP_TOP + STRIP_H + theme::space::SM;
const AXIS_H: f32 = 40.0;
const ROWS_TOP: f32 = AXIS_TOP + AXIS_H;
const ROW_H: f32 = 80.0;
const ROW_GAP: f32 = 8.0;
const CELL_GAP: f32 = 6.0;
const CELL_RAD: f32 = theme::space::XS;
/// The channel column: the logo tile (16:9 at the row's height) and the name beside it.
const CH_W: f32 = 340.0;
const TILE_W: f32 = (ROW_H - ROW_GAP) * 16.0 / 9.0;
const CELLS_X: f32 = MARGIN_X + CH_W + theme::space::SM;
const CELLS_W: f32 = SCR_W - MARGIN_X - CELLS_X;
const NOW_W: f32 = 3.0;
/// A programme cell's genre edge: its width and its inset from the cell's corners.
const EDGE_W: f32 = 4.0;
const EDGE_INSET: f32 = 10.0;
/// The setup page's controls.
const SETUP_LIST_TOP: f32 = 470.0;
const SETUP_ROW_H: f32 = 64.0;
const SETUP_ROW_GAP: f32 = theme::space::SM;
const FIELD_W: f32 = 900.0;
const FIELD_PAD: f32 = theme::space::MD;

/// How many channel rows fit.
pub fn visible_rows() -> usize {
    (((SCR_H - MARGIN_Y - ROWS_TOP) / ROW_H).floor() as usize).max(1)
}

// ---- the page's faces ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Setup,
    Loading,
    Guide,
    /// The channel studio ([`studio`]): opened from the guide, or the page itself when there is
    /// no Tunarr server and no kept channel but the library can suggest some.
    Studio,
}

impl Face {
    /// The face the store's state calls for. A virtual channel is a guide of its own: without a
    /// Tunarr server, or with one that failed, the guide still shows the channels the profile kept.
    pub fn of(view: LiveTvView<'_>, force_setup: bool) -> Face {
        if force_setup {
            return Face::Setup;
        }
        if !view.tunarr_configured() {
            return if view.has_virtuals() { Face::Guide } else { Face::Setup };
        }
        match view.status() {
            Status::Ready => Face::Guide,
            Status::Loading => Face::Loading,
            Status::Failed(_) | Status::Unconfigured if view.has_virtuals() => Face::Guide,
            Status::Failed(_) | Status::Unconfigured => Face::Setup,
        }
    }

    /// The face the page shows: [`Face::of`], with the studio when it is open, and in place of
    /// the Tunarr setup when there is no Tunarr server but channels can come from the library
    /// (suggested, or the library still being read for them).
    pub fn page(view: LiveTvView<'_>, force_setup: bool, studio_open: bool) -> Face {
        if force_setup {
            return Face::Setup;
        }
        if studio_open {
            return Face::Studio;
        }
        let face = Face::of(view, false);
        let library = view.configured() || view.virtuals().busy() || view.virtuals().catalog().is_some();
        if face == Face::Setup && !view.tunarr_configured() && library { Face::Studio } else { face }
    }
}

/// One setup row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Search,
    Found(usize),
    Address,
    Retry,
    Back,
    TurnOff,
}

/// The setup rows, top to bottom, for this view.
pub fn setup_items(view: LiveTvView<'_>, force_setup: bool) -> Vec<Item> {
    let mut items = vec![Item::Search];
    if let Discovery::Done(found) = view.discovery() {
        items.extend((0..found.len()).map(Item::Found));
    }
    items.push(Item::Address);
    if view.tunarr_configured() {
        if matches!(view.status(), Status::Failed(_)) {
            items.push(Item::Retry);
        }
        if force_setup && matches!(view.status(), Status::Ready) {
            items.push(Item::Back);
        }
        items.push(Item::TurnOff);
    }
    items
}

// ---- the screen ---------------------------------------------------------------------------------

pub struct LiveTvScreen {
    entry: EntryId,
    instance: InstanceId,
    cursor: Cursor,
    setup_sel: usize,
    /// The viewer asked to change the server from a loaded guide (Settings > Live TV opens the page
    /// in this state through `LiveTvReq`'s owner).
    force_setup: bool,
    editing: bool,
    address: TextBuffer,
    /// Digits typed on the remote, and when the last one arrived.
    typed: String,
    typed_at: u32,
    /// A LAN search has been started once for this visit (an unconfigured page starts one by
    /// itself, once).
    searched: bool,
    /// The channel last tuned (`Session::livetv_channel`), read once when the page mounts.
    remembered: String,
    /// The cursor has been put on [`Self::remembered`]'s row — once, the first time the guide has
    /// rows to put it on; after that the cursor is the viewer's.
    seated: bool,
    /// The genre the grid is narrowed to ([`filter`]).
    filter: Filter,
    /// The cursor is on the genre strip rather than in the grid, on chip [`Self::strip_sel`].
    on_strip: bool,
    strip_sel: usize,
    /// The lineup rows on show under [`Self::filter`] — derived (the lineup, the filter and the
    /// window decide it, [`filter::visible`]) and kept with what it was derived from, so the cursor
    /// can be re-seated on its channel when it changes.
    rows: Vec<usize>,
    rows_from: Option<(u64, i64, Filter)>,
    /// The strip's chips for the lineup at a revision ([`filter::chips`]), derived likewise.
    chips: Vec<Filter>,
    chips_from: Option<u64>,
    /// The Plex lookup's dwell: the [`Want::key`] the cursor rests on, since when, and when it was
    /// last asked (a lookup refused because another is in flight is asked again a dwell later).
    rest: Option<(String, u32, Option<u32>)>,
    /// The wall-clock minute last drawn, so the NOW marker moves once a minute and no more often.
    minute: i64,
    ground: plx_ui::widgets::PageGround,
    /// The channel studio's cursor and drafts ([`studio`]).
    studio: studio::Studio,
}

impl LiveTvScreen {
    pub fn new(entry: EntryId, instance: InstanceId) -> Self {
        let now = plx_base::wallclock::now_ms();
        Self {
            entry,
            instance,
            cursor: Cursor::new(now),
            setup_sel: 0,
            force_setup: false,
            editing: false,
            address: TextBuffer::new(String::new(), 0),
            typed: String::new(),
            typed_at: 0,
            searched: false,
            remembered: plx_plex::plex::session::peek().livetv_channel().to_owned(),
            seated: false,
            filter: Filter::All,
            on_strip: false,
            strip_sel: 0,
            rows: Vec::new(),
            rows_from: None,
            chips: Vec::new(),
            chips_from: None,
            rest: None,
            minute: now / 60_000,
            ground: plx_ui::widgets::PageGround::new(),
            studio: studio::Studio::default(),
        }
    }

    /// Open on the setup face even over a loaded guide (Settings > Live TV, *Change server*).
    pub fn show_setup(&mut self) {
        self.force_setup = true;
        self.setup_sel = 0;
    }

    /// Open the channel studio, on the card `want` (a suggestion's id, `surprise`) when given.
    pub fn show_studio(&mut self, want: Option<String>, view: LiveTvView<'_>) {
        self.force_setup = false;
        self.studio.open_on(want, view);
    }

    /// Carry out what the studio asked for.
    fn studio_out<H: LiveTvLike>(&mut self, outs: Vec<studio::Out>, view: LiveTvView<'_>, fx: &mut Effects<'_, H>) {
        for out in outs {
            match out {
                studio::Out::Store(cmd) => Self::store(LiveTvCmd::Virtual(cmd), fx),
                studio::Out::Tune(number) => {
                    if let Some(index) = view.lineup().index_of_number(&number) {
                        Self::ask(LiveTvReq::Tune { index }, fx);
                    }
                }
                // Back to the guide it was opened from — unless the studio is the page itself
                // (no Tunarr, no kept channel), when there is nothing under it to go back to.
                studio::Out::Close if self.studio.open && Face::page(view, self.force_setup, false) != Face::Studio
                    && Face::page(view, self.force_setup, false) != Face::Setup =>
                {
                    self.studio.open = false
                }
                studio::Out::Close => {
                    self.studio.open = false;
                    Self::ask(LiveTvReq::Back, fx)
                }
            }
        }
        fx.invalidate(Provenance::Input);
    }

    /// A key on the studio. `Handled::No` hands it to the engine (UP to the top strip).
    fn key_studio<H: LiveTvLike>(&mut self, key: Key, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let view = H::livetv(cx);
        let mut outs = Vec::new();
        let mine = self.studio.key(key, view, plx_base::wallclock::now_ms(), &mut outs);
        self.studio_out(outs, view, fx);
        if mine { Handled::Yes } else { Handled::No }
    }

    /// Ask the store to read the virtual channels (and the Tunarr guide when there is one) if
    /// they are stale — on every arrival of the page.
    fn refresh<H: LiveTvLike>(cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        if H::livetv(cx).tunarr_configured() {
            Self::store(LiveTvCmd::RefreshIfStale, fx);
        }
        Self::store(LiveTvCmd::Virtual(plx_data::vchannel::channels::VCmd::RefreshIfStale), fx);
    }

    fn key(&self) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem: ELEM }
    }

    fn focused<H: LiveTvLike>(&self, cx: &Cx<'_, H>) -> bool {
        cx.focus.current == Some(self.key())
    }

    fn store<H: LiveTvLike>(cmd: LiveTvCmd, fx: &mut Effects<'_, H>) {
        fx.push(Fx::App(AppFx::Store(StoreId::LiveTv, StoreCmd::LiveTv(cmd))));
    }

    fn ask<H: LiveTvLike>(req: LiveTvReq, fx: &mut Effects<'_, H>) {
        fx.push(Fx::App(AppFx::LiveTv(req)));
    }

    fn reseat<H: LiveTvLike>(&self, fx: &mut Effects<'_, H>) {
        fx.push(Fx::Deliver(
            MachineId::Instance(self.instance),
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::Elem(self.key()) })),
        ));
    }

    fn keyboard<H: LiveTvLike>(&mut self, up: bool, fx: &mut Effects<'_, H>) {
        if self.editing == up {
            return;
        }
        self.editing = up;
        fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Keyboard { up }));
        fx.invalidate(Provenance::Input);
    }

    /// The typed address, committed as the server — if it parses as one.
    fn commit_address<H: LiveTvLike>(&mut self, view: LiveTvView<'_>, fx: &mut Effects<'_, H>) {
        let typed = self.address.text().trim().to_owned();
        if let Some(origin) = plx_data::livetv::hdhr::normalise_address(&typed) {
            if origin != view.source() || !matches!(view.status(), Status::Ready) {
                Self::store(LiveTvCmd::Use { origin }, fx);
            }
            self.force_setup = false;
        }
    }

    fn face<H: LiveTvLike>(&self, cx: &Cx<'_, H>) -> Face {
        Face::page(H::livetv(cx), self.force_setup, self.studio.open)
    }

    // ---- the guide's derived rows ----------------------------------------------------------------

    /// The rows on show, as the cursor walks them.
    fn shown<'a>(&'a self, lineup: &'a Lineup) -> Rows<'a> {
        Rows::some(lineup, &self.rows)
    }

    /// Re-derive [`Self::rows`] and [`Self::chips`] when what they come from moved, keeping the
    /// cursor on its channel (or the next one shown) and the strip's cursor on a chip that exists.
    /// A filter whose chip the reloaded guide no longer offers falls back to *All*. `true` when
    /// the rows changed.
    fn derive(&mut self, view: LiveTvView<'_>) -> bool {
        let lineup = view.lineup();
        let revision = view.revision();
        if self.chips_from != Some(revision) {
            self.chips = filter::chips(lineup);
            self.chips_from = Some(revision);
            if !self.chips.contains(&self.filter) {
                self.filter = Filter::All;
            }
        }
        self.strip_sel = self.strip_sel.min(self.chips.len().saturating_sub(1));
        let from = (revision, self.cursor.window_ms, self.filter.clone());
        if self.rows_from.as_ref() == Some(&from) {
            return false;
        }
        let rows = filter::visible(lineup, &self.filter, self.cursor.window_ms, self.cursor.window_ms + grid::WINDOW_MS);
        self.rows_from = Some(from);
        if rows == self.rows {
            return false;
        }
        let was = self.rows.get(self.cursor.row).copied();
        self.rows = rows;
        self.cursor.row = filter::reseat(&self.rows, was);
        self.cursor.fit(Rows::some(lineup, &self.rows), visible_rows());
        true
    }

    /// The channel and airing under the cursor, when the cursor is in the grid.
    fn focus_airing<'a>(&self, lineup: &'a Lineup) -> Option<(&'a Channel, Option<&'a Airing>)> {
        let ch = lineup.channels.get(*self.rows.get(self.cursor.row)?)?;
        Some((ch, ch.airing_at(self.cursor.at_ms).map(|i| &ch.airings[i])))
    }

    /// What is known in the viewer's Plex library about the airing under the cursor.
    fn focus_match<'a>(&self, view: LiveTvView<'a>) -> Option<&'a Hit> {
        if self.on_strip {
            return None;
        }
        let (_, airing) = self.focus_airing(view.lineup())?;
        let want = Want::of(airing?)?;
        match view.plex_match(&want.key())? {
            PlexMatch::Found(hit) => Some(hit),
            PlexMatch::Looking | PlexMatch::None => None,
        }
    }

    /// Tune the channel under the cursor.
    fn tune<H: LiveTvLike>(&self, view: LiveTvView<'_>, fx: &mut Effects<'_, H>) -> Handled {
        if let Some(index) = self.shown(view.lineup()).lineup_index(self.cursor.row) {
            Self::ask(LiveTvReq::Tune { index }, fx);
        }
        Handled::Yes
    }

    /// OK on the setup row under the cursor.
    fn activate_setup<H: LiveTvLike>(&mut self, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        let view = H::livetv(cx);
        let items = setup_items(view, self.force_setup);
        let Some(item) = items.get(self.setup_sel.min(items.len().saturating_sub(1))).copied() else { return };
        match item {
            Item::Search => {
                self.searched = true;
                Self::store(LiveTvCmd::Discover, fx);
            }
            Item::Found(i) => {
                if let Discovery::Done(found) = view.discovery() {
                    if let Some(f) = found.get(i) {
                        Self::store(LiveTvCmd::Use { origin: f.origin.clone() }, fx);
                        self.force_setup = false;
                    }
                }
            }
            Item::Address => {
                if self.editing {
                    self.keyboard(false, fx);
                    self.commit_address(view, fx);
                } else {
                    if self.address.text().is_empty() && view.tunarr_configured() {
                        let source = view.source().to_owned();
                        let len = source.len();
                        self.address = TextBuffer::new(source, len);
                    }
                    self.keyboard(true, fx);
                }
            }
            Item::Retry => Self::store(LiveTvCmd::Refresh, fx),
            Item::Back => self.force_setup = false,
            Item::TurnOff => {
                Self::store(LiveTvCmd::Forget, fx);
                self.force_setup = false;
                self.setup_sel = 0;
            }
        }
        fx.invalidate(Provenance::Input);
    }

    /// A strip pill pressed while this page owns input.
    fn activate_strip<H: LiveTvLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) -> Handled {
        let Some(tab) = elem.checked_sub(plx_ui::dispatch::STRIP_BASE) else { return Handled::No };
        let req = match tab {
            0 => LiveTvReq::Tab(HomeTab::Home),
            1 => LiveTvReq::Tab(HomeTab::Movies),
            2 => LiveTvReq::Tab(HomeTab::Shows),
            3 => LiveTvReq::Tab(HomeTab::Search),
            4 => LiveTvReq::Account,
            5 => return Handled::Yes,
            _ => return Handled::No,
        };
        self.keyboard(false, fx);
        Self::ask(req, fx);
        Handled::Yes
    }

    /// A key on the genre strip.
    fn key_chips<H: LiveTvLike>(&mut self, key: Key, view: LiveTvView<'_>, measure: &dyn plx_machine::machine::Measure, fx: &mut Effects<'_, H>) -> Handled {
        // The chips that fit, then the studio's pill.
        let n = draw::strip_fit(&self.chips, measure) + 1;
        match key {
            // The top strip's, through the engine's link.
            Key::Up => return Handled::No,
            Key::Down => {
                if self.rows.is_empty() {
                    return Handled::Yes;
                }
                self.on_strip = false;
            }
            Key::Left => {
                if self.strip_sel == 0 {
                    return Handled::Yes;
                }
                self.strip_sel -= 1;
            }
            Key::Right => {
                if self.strip_sel + 1 >= n {
                    return Handled::Yes;
                }
                self.strip_sel += 1;
            }
            Key::Ok if self.strip_sel + 1 == n => {
                self.show_studio(None, view);
            }
            Key::Ok => {
                let Some(chosen) = self.chips.get(self.strip_sel).cloned() else { return Handled::Yes };
                if chosen != self.filter {
                    self.filter = chosen;
                    self.derive(view);
                }
            }
            Key::Back => {
                Self::ask(LiveTvReq::Back, fx);
                return Handled::Yes;
            }
            Key::Other => return Handled::No,
        }
        fx.invalidate(Provenance::Input);
        Handled::Yes
    }

    /// A key while this page's element holds focus on the guide. `Handled::No` hands it to the
    /// engine (the top strip's link; OK, which the engine arms as a holdable press).
    fn key_guide<H: LiveTvLike>(&mut self, key: Key, sym: u32, wcode: u32, now_ms: u32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let view = H::livetv(cx);
        self.derive(view);
        let lineup = view.lineup();
        let wall = plx_base::wallclock::now_ms();
        let visible = visible_rows();
        if let Some(dir) = plx_ui::consts::page_dir(sym, wcode) {
            self.on_strip = false;
            if self.cursor.page(Rows::some(lineup, &self.rows), dir, visible) {
                fx.invalidate(Provenance::Input);
            }
            return Handled::Yes;
        }
        if let Some(d) = digit_of(sym, wcode) {
            if now_ms.wrapping_sub(self.typed_at) > DIGIT_MS {
                self.typed.clear();
            }
            if self.typed.len() < 6 {
                self.typed.push(d as char);
            }
            self.typed_at = now_ms;
            if let Some(index) = lineup.index_for_typed(&self.typed) {
                // A channel the filter hides is still the channel asked for: the filter gives way.
                if !self.rows.contains(&index) {
                    self.filter = Filter::All;
                    self.derive(view);
                }
                if let Some(row) = self.rows.iter().position(|&r| r == index) {
                    self.on_strip = false;
                    self.cursor.go_to_row(row, visible);
                }
            }
            fx.invalidate(Provenance::Input);
            return Handled::Yes;
        }
        if self.on_strip {
            return self.key_chips(key, view, cx.measure, fx);
        }
        let rows = Rows::some(lineup, &self.rows);
        let moved = match key {
            Key::Up => {
                if !self.cursor.step_row(rows, -1, visible) {
                    // The first row's UP reaches the genre strip, on the chip that is in force.
                    self.on_strip = true;
                    self.strip_sel = self.chips.iter().position(|c| *c == self.filter).unwrap_or(0);
                }
                true
            }
            Key::Down => self.cursor.step_row(rows, 1, visible),
            Key::Left => self.cursor.left(rows, wall),
            Key::Right => self.cursor.right(rows, wall),
            // Armed by the engine as a press: its release tunes, its hold opens the Plex match.
            Key::Ok => return if self.rows.is_empty() { Handled::Yes } else { Handled::No },
            Key::Back => {
                Self::ask(LiveTvReq::Back, fx);
                return Handled::Yes;
            }
            Key::Other => return Handled::No,
        };
        if moved {
            // A sideways move can slide the window, which can change the rows a filter keeps.
            self.derive(view);
            fx.invalidate(Provenance::Input);
        }
        Handled::Yes
    }

    fn key_setup<H: LiveTvLike>(&mut self, key: Key, sym: u32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let view = H::livetv(cx);
        if self.editing {
            let edit = match (key, sym) {
                (Key::Left, _) => Some(TextEdit::Left),
                (Key::Right, _) => Some(TextEdit::Right),
                (_, plx_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                (_, plx_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
                _ => None,
            };
            if let Some(edit) = edit {
                self.address.edit(&edit);
                fx.invalidate(Provenance::Input);
                return Handled::Yes;
            }
            match key {
                Key::Ok => {
                    self.keyboard(false, fx);
                    self.commit_address(view, fx);
                    return Handled::Yes;
                }
                Key::Back => {
                    self.keyboard(false, fx);
                    return Handled::Yes;
                }
                _ => {}
            }
        }
        let n = setup_items(view, self.force_setup).len();
        match key {
            Key::Up => {
                if self.setup_sel == 0 {
                    self.keyboard(false, fx);
                    return Handled::No;
                }
                self.setup_sel -= 1;
                self.keyboard(false, fx);
            }
            Key::Down => {
                if self.setup_sel + 1 < n {
                    self.setup_sel += 1;
                    self.keyboard(false, fx);
                }
            }
            Key::Ok => self.activate_setup(cx, fx),
            Key::Back => {
                if self.force_setup && matches!(view.status(), Status::Ready) {
                    self.force_setup = false;
                } else {
                    Self::ask(LiveTvReq::Back, fx);
                }
            }
            Key::Left | Key::Right | Key::Other => return Handled::No,
        }
        fx.invalidate(Provenance::Input);
        Handled::Yes
    }

    fn tick<H: LiveTvLike>(&mut self, now_ms: u32, dt: f32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        let view = H::livetv(cx);
        let wall = plx_base::wallclock::now_ms();
        if self.cursor.tick(wall) {
            // A following cursor moves every frame; only the minute is drawn.
        }
        if self.derive(view) {
            fx.invalidate(Provenance::Lifecycle);
        }
        self.cursor.fit(Rows::some(view.lineup(), &self.rows), visible_rows());
        if !self.seated && !view.lineup().is_empty() {
            self.seated = true;
            if let Some(row) = view.lineup().index_of_number(&self.remembered).and_then(|i| self.rows.iter().position(|&r| r == i)) {
                self.cursor.go_to_row(row, visible_rows());
                fx.invalidate(Provenance::Lifecycle);
            }
        }
        let minute = wall / 60_000;
        if minute != self.minute {
            self.minute = minute;
            fx.invalidate(Provenance::Lifecycle);
        }
        if !self.typed.is_empty() && now_ms.wrapping_sub(self.typed_at) > DIGIT_MS {
            self.typed.clear();
            fx.invalidate(Provenance::Lifecycle);
        }
        let n = setup_items(view, self.force_setup).len();
        if self.setup_sel >= n {
            self.setup_sel = n.saturating_sub(1);
        }
        let face = self.face(cx);
        if face == Face::Studio {
            // Once on screen the studio stays until it is left: a channel kept from it must not
            // swap the page for the guide that channel just made possible.
            self.studio.open = true;
            let mut outs = Vec::new();
            if self.studio.tick(view, now_ms, wall, self.focused(cx), &mut outs) {
                fx.invalidate(Provenance::Lifecycle);
            }
            if !outs.is_empty() {
                self.studio_out(outs, view, fx);
            }
        }
        // An unconfigured page looks for Tunarr by itself, once per visit — while it shows setup.
        if face == Face::Setup && !view.tunarr_configured() && !self.searched && matches!(view.discovery(), Discovery::Idle) {
            self.searched = true;
            Self::store(LiveTvCmd::Discover, fx);
        }
        let guide = face == Face::Guide;
        self.key_ground(view, face, dt);
        self.dwell(view, guide && self.focused(cx), now_ms, fx);
    }

    /// Dissolve the page ground toward the focused airing's artwork (its programme picture, else
    /// its channel's logo — exactly the picture the info pane draws, at the same key): HELD while
    /// that picture is on its way, the flat surface when the airing has no picture at all.
    fn key_ground(&mut self, view: LiveTvView<'_>, face: Face, dt: f32) {
        if face == Face::Studio {
            let rows = studio::rows(view, self.studio.made());
            let card = self.studio.focus(&rows);
            let art = card.and_then(|c| studio_draw::card_art(&c, self.studio.schedule(&c, view), plx_base::wallclock::now_ms()));
            match art {
                Some((srv, path)) => {
                    let corners = plx_ui::tex::corners_on(srv, path, studio_draw::CARD_W as i32 * 2, studio_draw::CARD_H as i32 * 2, false);
                    self.ground.key(corners, plx_ui::widgets::PageGround::CARD_W, dt);
                }
                None => self.ground.key_target([theme::SURFACE_APP; 4], dt),
            }
            return;
        }
        let art = if face == Face::Guide { self.focus_airing(view.lineup()).map(|(ch, a)| draw::info_art(ch, a)) } else { None };
        match art {
            Some(Some(url)) => {
                let corners = { let (srv, path) = plx_ui::tex::art_source(url); plx_ui::tex::corners_on(srv, path, ART_MAX_W as i32, ART_H as i32, false) };
                self.ground.key(corners, plx_ui::widgets::PageGround::CARD_W, dt);
            }
            _ => self.ground.key_target([theme::SURFACE_APP; 4], dt),
        }
    }

    /// The Plex lookup's dwell: once the cursor has rested on one airing for [`MATCH_DWELL_MS`],
    /// ask the store to look it up (it answers from its cache, or starts one lookup at a time).
    fn dwell<H: LiveTvLike>(&mut self, view: LiveTvView<'_>, on: bool, now_ms: u32, fx: &mut Effects<'_, H>) {
        let want = if on && !self.on_strip {
            self.focus_airing(view.lineup()).and_then(|(_, a)| a).and_then(Want::of)
        } else {
            None
        };
        let Some(want) = want else {
            self.rest = None;
            return;
        };
        let key = want.key();
        match &mut self.rest {
            Some((k, since, asked)) if *k == key => {
                let due = match asked {
                    Some(at) => now_ms.wrapping_sub(*at) >= MATCH_DWELL_MS,
                    None => now_ms.wrapping_sub(*since) >= MATCH_DWELL_MS,
                };
                if due && view.plex_match(&key).is_none() {
                    *asked = Some(now_ms);
                    Self::store(LiveTvCmd::Match(want), fx);
                }
            }
            _ => self.rest = Some((key, now_ms, None)),
        }
    }
}

/// Remote number key → digit, from the key's ASCII `sym` only: `wcode` is a scancode, where 48–57
/// are punctuation (`consts::is_bound`'s doc, `docs/remote-keys.md` §9).
fn digit_of(sym: u32, _wcode: u32) -> Option<u8> {
    Some(sym).filter(|v| (48..=57).contains(v)).map(|v| v as u8)
}

impl<H: LiveTvLike> Machine<H> for LiveTvScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                Self::refresh(cx, fx);
                self.reseat(fx);
                Handled::Yes
            }
            ScreenEvent::Uncover | ScreenEvent::Resume => {
                Self::refresh(cx, fx);
                Handled::Yes
            }
            ScreenEvent::App(crate::registry::AppMsg::LiveTvSetup) => {
                self.studio.open = false;
                self.show_setup();
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::App(crate::registry::AppMsg::LiveTvMake { recipe, why }) => {
                self.force_setup = false;
                self.studio.open_make((**recipe).clone(), why.clone(), H::livetv(cx));
                self.reseat(fx);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::App(crate::registry::AppMsg::LiveTvStudio(want)) => {
                self.show_studio(Some(want.clone()), H::livetv(cx));
                self.reseat(fx);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend => {
                self.keyboard(false, fx);
                Handled::Yes
            }
            ScreenEvent::StoreChanged(store, _) if *store == StoreId::LiveTv.ord() => {
                let view = H::livetv(cx);
                self.derive(view);
                self.cursor.fit(Rows::some(view.lineup(), &self.rows), visible_rows());
                fx.invalidate(Provenance::Landing(MachineId::Store(StoreId::LiveTv.ord())));
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.tick(t.ms, t.dt(), cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                if *elem == ELEM {
                    return match self.face(cx) {
                        Face::Setup => {
                            self.activate_setup(cx, fx);
                            Handled::Yes
                        }
                        Face::Guide if self.on_strip => self.key_chips(Key::Ok, H::livetv(cx), cx.measure, fx),
                        Face::Guide => self.tune(H::livetv(cx), fx),
                        Face::Loading => Handled::Yes,
                        Face::Studio => self.key_studio(Key::Ok, cx, fx),
                    };
                }
                self.activate_strip(*elem, fx)
            }
            ScreenEvent::PressCommit(_) => match cx.focus.current {
                Some(key) if key.elem >= plx_ui::dispatch::STRIP_BASE => self.activate_strip(key.elem, fx),
                Some(key) if key == self.key() && self.face(cx) == Face::Guide && !self.on_strip => self.tune(H::livetv(cx), fx),
                _ => Handled::No,
            },
            // A held OK on an airing that is in the viewer's Plex library opens it there; on any
            // other airing the hold is just a slow press, and tunes.
            ScreenEvent::PressHold(_) => match cx.focus.current {
                Some(key) if key == self.key() && self.face(cx) == Face::Guide && !self.on_strip => {
                    match self.focus_match(H::livetv(cx)) {
                        Some(hit) => Self::ask(LiveTvReq::Detail { sid: hit.sid, rk: hit.rk.clone() }, fx),
                        None => {
                            self.tune(H::livetv(cx), fx);
                        }
                    }
                    Handled::Yes
                }
                _ => Handled::No,
            },
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::SystemKeyboard(up) => {
                    let was = self.editing;
                    self.editing = *up;
                    if was && !*up {
                        // The television's keyboard closed (its Done key): take what was typed.
                        self.commit_address(H::livetv(cx), fx);
                    }
                    fx.invalidate(Provenance::Input);
                    Handled::Yes
                }
                InputKind::Text(edit) if self.editing => {
                    self.address.edit(edit);
                    fx.invalidate(Provenance::Input);
                    Handled::Yes
                }
                InputKind::Key { key, sym, wcode, edge, .. } if *edge != Edge::Up => {
                    if !self.focused(cx) && !self.editing {
                        return Handled::No;
                    }
                    match self.face(cx) {
                        Face::Guide => self.key_guide(*key, *sym, *wcode, input.at.ms, cx, fx),
                        Face::Setup => self.key_setup(*key, *sym, cx, fx),
                        Face::Studio => self.key_studio(*key, cx, fx),
                        Face::Loading => match key {
                            Key::Back => {
                                Self::ask(LiveTvReq::Back, fx);
                                Handled::Yes
                            }
                            Key::Up => Handled::No,
                            _ => Handled::Yes,
                        },
                    }
                }
                _ => Handled::No,
            },
            _ => Handled::No,
        }
    }
}

impl<H: LiveTvLike> Focusable<H> for LiveTvScreen {
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        // On the guide the element is a CARD to the engine, so OK arms a press that can be HELD
        // (the Plex match); a setup row activates on the down edge like any bare element.
        let elem = if self.face(cx) == Face::Guide && !self.on_strip { ElemKind::Card } else { ElemKind::Bare };
        out.push(GroupSpec {
            id: GROUP,
            kind: GroupKind::Free,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent: Rect::new(MARGIN_X, TOP, SCR_W - 2.0 * MARGIN_X, SCR_H - TOP - MARGIN_Y),
            len: 1,
            elem,
        });
    }
    fn group_of(&self, key: &u32, _cx: &Cx<'_, H>) -> Option<GroupId> {
        (*key == ELEM).then_some(GROUP)
    }
    fn neighbour(&self, _key: FocusKey<u32>, _dir: Dir, _cx: &Cx<'_, H>) -> Step<u32> {
        Step::Edge
    }
    fn place(&self, key: &u32, cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        if *key != ELEM {
            return None;
        }
        let rect = match self.face(cx) {
            Face::Guide if self.on_strip => draw::strip_rects(&self.chips, cx.measure)
                .get(self.strip_sel)
                .copied()
                .unwrap_or_else(|| draw::studio_pill_rect(cx.measure)),
            Face::Studio => self.studio_focus_rect(H::livetv(cx), cx.measure),
            Face::Guide => self.focus_cell_rect(H::livetv(cx)),
            Face::Setup => setup_row_rect(self.setup_sel),
            Face::Loading => Rect::new(MARGIN_X, TOP, SCR_W - 2.0 * MARGIN_X, ROW_H),
        };
        Some(Placed { rect, rest_rect: rect, clip: Rect::FULL, index: None })
    }
    fn reconcile(&self, want: FocusKey<u32>, _cx: &Cx<'_, H>) -> FocusKey<u32> {
        want
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, H>) -> FocusKey<u32> {
        self.key()
    }
}

impl LogicalState for LiveTvScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.instance.0);
        c.u64(self.cursor.row as u64).u64(self.cursor.at_ms as u64).u64(self.cursor.window_ms as u64)
            .u64(self.cursor.top as u64).bool(self.cursor.follow);
        c.u64(self.setup_sel as u64).bool(self.force_setup).bool(self.editing)
            .str(self.address.text()).str(&self.typed).bool(self.searched)
            .str(&self.remembered).bool(self.seated);
        c.str(&self.filter.canon()).bool(self.on_strip).u64(self.strip_sel as u64);
        c.str(&self.studio.canon());
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "livetv row={} top={} setup_sel={} force_setup={} editing={} filter={} on_strip={} strip_sel={} studio={}",
            self.cursor.row, self.cursor.top, self.setup_sel, self.force_setup, self.editing,
            self.filter.canon(), self.on_strip, self.strip_sel, self.studio.canon()
        ));
    }
}

impl<H: LiveTvLike> Screen<H> for LiveTvScreen {
    fn name(&self) -> &'static str {
        crate::registry::word::LIVETV
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        // A page's first act is the frame clear (`gfx::frame_clear`'s doc): the ground below draws
        // nothing while it rests on the clear colour, so without it every repaint lands on the last
        // frame's pixels — the moved cursor's old cell stays lit and the info pane's text stacks.
        plx_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        let p = f.painter.alpha(f.page_alpha);
        self.ground.draw(p, Rect::FULL);
        let view = H::livetv(f.cx);
        let focused = self.focused(f.cx);
        match self.face(f.cx) {
            Face::Setup => self.draw_setup(p, view, focused, f.cx.tick.ms, f.measure),
            Face::Loading => {
                // placeholder-exempt: builds the value only; counted where drawn (StatusOverlay, Working)
                StatusOverlay::new(guide_frame(), plx_platform::i18n::msg::livetv_loading_c(), StatusKind::Working)
                    .phase(f.cx.tick.ms)
                    .draw_measured(&plx_ui::Env::inert(), p, f.measure);
            }
            Face::Guide => self.draw_guide(p, view, focused, f.measure),
            Face::Studio => self.draw_studio(p, view, focused, f.cx.tick.ms, f.measure),
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn memory(&self) -> PageMemory {
        PageMemory::None
    }
    fn links(&self, out: &mut Vec<Link>) {
        out.push(Link { from: STRIP, dir: Dir::Down, to: GROUP });
        out.push(Link { from: GROUP, dir: Dir::Up, to: STRIP });
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn guide_frame() -> Rect {
    Rect::new(MARGIN_X, TOP, SCR_W - 2.0 * MARGIN_X, SCR_H - TOP - MARGIN_Y)
}

fn setup_row_rect(i: usize) -> Rect {
    Rect::new(MARGIN_X, SETUP_LIST_TOP + i as f32 * (SETUP_ROW_H + SETUP_ROW_GAP), FIELD_W, SETUP_ROW_H)
}

fn x_of(t: i64, window: i64) -> f32 {
    CELLS_X + (t - window) as f32 / grid::WINDOW_MS as f32 * CELLS_W
}

fn row_y(row: usize, top: usize) -> f32 {
    ROWS_TOP + (row.saturating_sub(top)) as f32 * ROW_H
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::livetv::LiveTvState;

    #[test]
    fn the_face_follows_the_store() {
        let mut s = LiveTvState::default();
        assert_eq!(Face::of(s.view(), false), Face::Setup, "no server: setup");
        s.run(LiveTvCmd::Restore { origin: "192.0.2.20".into() }, 0);
        assert_eq!(Face::of(s.view(), false), Face::Loading, "a restored server loads at once");
        s.install_for_test("http://192.0.2.20:8000", Default::default(), 0);
        assert_eq!(Face::of(s.view(), false), Face::Guide);
        assert_eq!(Face::of(s.view(), true), Face::Setup, "change server");
    }

    #[test]
    fn setup_rows_offer_what_the_state_allows() {
        let mut s = LiveTvState::default();
        assert_eq!(setup_items(s.view(), false), [Item::Search, Item::Address]);
        s.install_for_test("http://192.0.2.20:8000", Default::default(), 0);
        assert_eq!(setup_items(s.view(), true), [Item::Search, Item::Address, Item::Back, Item::TurnOff]);
    }

    #[test]
    fn digits_read_from_the_ascii_sym_only() {
        assert_eq!(digit_of(b'7' as u32, 0), Some(b'7'));
        // 48 in `wcode` is a scancode (`]`), not the digit 0 — docs/remote-keys.md §9
        assert_eq!(digit_of(0, b'0' as u32), None);
        assert_eq!(digit_of(13, 0), None);
    }

    #[test]
    fn the_grid_fits_rows_under_the_axis() {
        let n = visible_rows();
        assert!(n >= 5, "{n}");
        assert!(ROWS_TOP + n as f32 * ROW_H <= SCR_H - MARGIN_Y + 0.5);
        assert!(TOP + ART_H + theme::space::XS + plx_ui::widgets::KeyHint::height() <= STRIP_TOP, "the Plex hint clears the strip");
        assert!(MARGIN_X + INFO_W + theme::space::LG <= SCR_W - MARGIN_X - ART_MAX_W, "the text column clears the widest art");
    }

    const MIN: i64 = 60_000;

    fn guide(now: i64) -> plx_data::livetv::guide::Lineup {
        use plx_data::livetv::guide::{Airing, Channel, Lineup};
        let window = grid::floor_slot(now);
        let a = |title: &str, start: i64, len: i64, cats: &[&str]| Airing {
            title: title.into(),
            start_ms: window + start * MIN,
            stop_ms: window + (start + len) * MIN,
            categories: cats.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        };
        let ch = |n: &str, airings: Vec<Airing>| Channel { number: n.into(), name: format!("Channel {n}"), airings, ..Default::default() };
        Lineup {
            channels: vec![
                ch("1", vec![a("Film", 0, 120, &["Movie"])]),
                ch("2", vec![a("Match", 0, 60, &["Sports"]), a("Later film", 60, 60, &["Movie"])]),
                ch("3", vec![a("Bulletin", 0, 120, &["News"])]),
                ch("4", vec![a("Another film", 0, 120, &["Movie"])]),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_genre_filter_hides_channels_without_a_match_and_keeps_the_cursor_on_its_channel() {
        let now = plx_base::wallclock::now_ms();
        let mut s = LiveTvState::default();
        s.install_for_test("http://192.0.2.20:8000", guide(now), now);
        let mut page = LiveTvScreen::new(EntryId(1), InstanceId(1));
        page.derive(s.view());
        assert_eq!(page.rows, [0, 1, 2, 3], "All shows every channel");
        assert_eq!(page.chips[1], Filter::Genre(plx_data::livetv::guide::Genre::Movie));
        page.cursor.go_to_row(3, visible_rows());
        page.filter = Filter::Genre(plx_data::livetv::guide::Genre::Movie);
        assert!(page.derive(s.view()));
        assert_eq!(page.rows, [0, 1, 3], "the news channel has no film in the window");
        assert_eq!(page.rows[page.cursor.row], 3, "the cursor stayed on channel 4");
        page.filter = Filter::Genre(plx_data::livetv::guide::Genre::News);
        page.derive(s.view());
        assert_eq!((page.rows.clone(), page.cursor.row), (vec![2], 0), "its channel hidden, the cursor takes the last shown");
        assert!(!page.derive(s.view()), "nothing moved: nothing re-derived");
        page.filter = Filter::Category("Cooking".into());
        page.chips_from = None;
        page.derive(s.view());
        assert_eq!(page.filter, Filter::All, "a filter the guide no longer offers gives way to All");
    }

    #[test]
    fn the_info_pane_prefers_the_programme_picture_and_joins_the_episode_facts() {
        use plx_data::livetv::guide::{Airing, Channel};
        let ch = Channel { icon: "http://192.0.2.20/logo.png".into(), ..Default::default() };
        let with = Airing { icon: "http://192.0.2.20/poster.jpg".into(), ..Default::default() };
        assert_eq!(draw::info_art(&ch, Some(&with)), Some("http://192.0.2.20/poster.jpg"));
        assert_eq!(draw::info_art(&ch, Some(&Airing::default())), Some("http://192.0.2.20/logo.png"));
        assert_eq!(draw::info_art(&Channel::default(), None), None);
        let ep = Airing { episode: "S02E05".into(), sub_title: "Dance Mode".into(), ..Default::default() };
        assert_eq!(draw::episode_line(&ep), "S02E05 \u{b7} Dance Mode");
        assert_eq!(draw::episode_line(&Airing { sub_title: "Pilot".into(), ..Default::default() }), "Pilot");
        assert_eq!(draw::episode_line(&Airing::default()), "");
    }

    /// Every chip the app names itself — *All* and the six genres — fits the strip whole, in every
    /// shipped language, at the device's whole-pixel advances.
    #[test]
    fn livetv_chip_labels_fit() {
        use plx_base::fontcov::advances::ShippedMeasure;
        use plx_data::livetv::guide::Genre;
        let mut chips = vec![Filter::All];
        chips.extend(Genre::ALL.iter().map(|g| Filter::Genre(*g)));
        for language in plx_platform::i18n::SHIPPED {
            let _guard = plx_platform::i18n::language_on_this_thread_for_test(language);
            assert_eq!(draw::strip_fit(&chips, &ShippedMeasure), chips.len(), "{language:?}");
            for chip in &chips {
                let label = chip.label();
                let w = plx_machine::machine::Measure::width_str(&ShippedMeasure, &label, theme::size::CAPTION, true);
                assert!(w <= CHIP_LABEL_MAX, "{language:?} {label} is {w}px");
            }
        }
    }
}
