//! **The Live TV page** — the configured Tunarr server's channel guide, or, when there is none (or
//! it cannot be loaded), the setup that finds one. A peer of Home, the Library and Search on the
//! top strip (`AppArg::LiveTv`). The data is `plx_data::livetv` (the store's view through
//! [`LiveTvLike`]); tuning a channel is a request the loop performs (`LiveTvReq::Tune`), because it
//! needs the playback session and the adapter (§2.1).
//!
//! Three faces, chosen from the store every frame ([`Face::of`]):
//!
//! * **Setup** — no server, a server that failed to load, or the viewer asked to change it: an
//!   explanation, *Search the network* (SSDP, `LiveTvCmd::Discover`), one row per Tunarr found, an
//!   address field on the television's own keyboard, and — when a server is configured — *Try
//!   again* and *Turn off Live TV*.
//! * **Loading** — the first load of a configured server.
//! * **Guide** — an info pane for the airing under the cursor over a grid of channel rows against
//!   a two-hour time axis with a NOW marker. The cursor is a moment ([`grid::Cursor`]); OK tunes
//!   the focused channel, digits jump to a channel number, CH▲/▼ page.
//!
//! **Focus.** The page declares ONE focusable element to the engine ([`ELEM`]) and keeps its own
//! cursor inside it — the guide's moment-and-row, or the setup's row — the way the person page's
//! biography keeps its page (`person_bio.rs`). An UP the cursor cannot take (the first guide row,
//! the first setup row) is returned to the engine, whose `Link` carries it to the strip; the
//! strip's DOWN comes back the same way.

pub mod grid;

use std::borrow::Cow;

use crate::registry::{AppFx, HomeTab, LiveTvLike, LiveTvReq, PageMemory};
use grid::Cursor;
use plx_data::livetv::{Discovery, LiveTvCmd, LiveTvView, Status};
use plx_data::stores::{StoreCmd, StoreId};
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind,
    InstanceId, Key, LogicalState, Machine, MachineId, TextEdit,
};
use plx_machine::present::Provenance;
use plx_ui::consts::{MARGIN_X, MARGIN_Y, SCR_H, SCR_W};
use plx_ui::frame::Budget;
use plx_ui::label::{HAlign, Label};
use plx_ui::screen::{
    At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Enter, FocusTarget, Focusable, GroupKind,
    GroupSpec, Link, Placed, RenderStrategy, Screen, ScreenEvent, Seat, Step,
};
use plx_ui::text_buffer::TextBuffer;
use plx_ui::text_view::TextView;
use plx_ui::widgets::{StatusKind, StatusOverlay, TabPill};
use plx_ui::{theme, Painter, Rect, View};

/// The page's one engine element, and its group.
pub const ELEM: u32 = 1;
const GROUP: GroupId = GroupId(0x4C54_5600);
const STRIP: GroupId = plx_ui::containers::tabs::STRIP;
/// The strip pill this page answers to (`app::chrome`'s Live TV key).
pub const STRIP_LIVETV_ELEM: u32 = plx_ui::dispatch::STRIP_BASE + 5;

/// How long a typed channel number waits for its next digit.
const DIGIT_MS: u32 = 1_500;

/// The fields [`LiveTvScreen`] canonicalises, for the recorder's shape pin (§5.4).
pub const SHAPE: &str = "LiveTvScreen{entry:u32,instance:u32,cursor:{row:u64,at:i64,window:i64,top:u64,follow:bool},setup_sel:u64,force_setup:bool,editing:bool,address:str,typed:str,searched:bool,remembered:str,seated:bool}";

// ---- geometry ----------------------------------------------------------------------------------

const TOP: f32 = plx_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
/// The guide's info pane: channel line, title, facts, two lines of description.
const INFO_W: f32 = 1_400.0;
/// The time axis band and the rows under it.
const AXIS_TOP: f32 = 432.0;
const AXIS_H: f32 = 40.0;
const ROWS_TOP: f32 = AXIS_TOP + AXIS_H;
const ROW_H: f32 = 80.0;
const ROW_GAP: f32 = 8.0;
const CELL_GAP: f32 = 6.0;
const CELL_RAD: f32 = theme::space::XS;
const CH_W: f32 = 280.0;
const CELLS_X: f32 = MARGIN_X + CH_W + theme::space::SM;
const CELLS_W: f32 = SCR_W - MARGIN_X - CELLS_X;
const NOW_W: f32 = 3.0;
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
}

impl Face {
    pub fn of(view: LiveTvView<'_>, force_setup: bool) -> Face {
        if !view.configured() || force_setup {
            return Face::Setup;
        }
        match view.status() {
            Status::Ready => Face::Guide,
            Status::Loading => Face::Loading,
            Status::Failed(_) | Status::Unconfigured => Face::Setup,
        }
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
    if view.configured() {
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
    /// The wall-clock minute last drawn, so the NOW marker moves once a minute and no more often.
    minute: i64,
    ground: plx_ui::widgets::PageGround,
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
            minute: now / 60_000,
            ground: plx_ui::widgets::PageGround::new(),
        }
    }

    /// Open on the setup face even over a loaded guide (Settings > Live TV, *Change server*).
    pub fn show_setup(&mut self) {
        self.force_setup = true;
        self.setup_sel = 0;
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
        Face::of(H::livetv(cx), self.force_setup)
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
                    if self.address.text().is_empty() && view.configured() {
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

    /// A key while this page's element holds focus. `Handled::No` hands it to the engine (the
    /// strip link, the strip's own keys).
    fn key_guide<H: LiveTvLike>(&mut self, key: Key, sym: u32, wcode: u32, now_ms: u32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let view = H::livetv(cx);
        let lineup = view.lineup();
        let wall = plx_base::wallclock::now_ms();
        let visible = visible_rows();
        if let Some(dir) = plx_ui::consts::page_dir(sym, wcode) {
            if self.cursor.page(lineup, dir, visible) {
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
            if let Some(row) = lineup.index_for_typed(&self.typed) {
                self.cursor.go_to_row(row, visible);
            }
            fx.invalidate(Provenance::Input);
            return Handled::Yes;
        }
        let moved = match key {
            Key::Up => {
                if !self.cursor.step_row(lineup, -1, visible) {
                    return Handled::No;
                }
                true
            }
            Key::Down => self.cursor.step_row(lineup, 1, visible),
            Key::Left => self.cursor.left(lineup, wall),
            Key::Right => self.cursor.right(lineup, wall),
            Key::Ok => {
                if !lineup.is_empty() {
                    Self::ask(LiveTvReq::Tune { index: self.cursor.row.min(lineup.len() - 1) }, fx);
                }
                return Handled::Yes;
            }
            Key::Back => {
                Self::ask(LiveTvReq::Back, fx);
                return Handled::Yes;
            }
            Key::Other => return Handled::No,
        };
        if moved {
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

    fn tick<H: LiveTvLike>(&mut self, now_ms: u32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        let view = H::livetv(cx);
        let wall = plx_base::wallclock::now_ms();
        if self.cursor.tick(wall) {
            // A following cursor moves every frame; only the minute is drawn.
        }
        self.cursor.fit(view.lineup(), visible_rows());
        if !self.seated && !view.lineup().is_empty() {
            self.seated = true;
            if let Some(row) = view.lineup().index_of_number(&self.remembered) {
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
        // An unconfigured page looks for Tunarr by itself, once per visit.
        if !view.configured() && !self.searched && matches!(view.discovery(), Discovery::Idle) {
            self.searched = true;
            Self::store(LiveTvCmd::Discover, fx);
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
                if H::livetv(cx).configured() {
                    Self::store(LiveTvCmd::RefreshIfStale, fx);
                }
                self.reseat(fx);
                Handled::Yes
            }
            ScreenEvent::Uncover | ScreenEvent::Resume => {
                if H::livetv(cx).configured() {
                    Self::store(LiveTvCmd::RefreshIfStale, fx);
                }
                Handled::Yes
            }
            ScreenEvent::App(crate::registry::AppMsg::LiveTvSetup) => {
                self.show_setup();
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend => {
                self.keyboard(false, fx);
                Handled::Yes
            }
            ScreenEvent::StoreChanged(store, _) if *store == StoreId::LiveTv.ord() => {
                self.cursor.fit(H::livetv(cx).lineup(), visible_rows());
                fx.invalidate(Provenance::Landing(MachineId::Store(StoreId::LiveTv.ord())));
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.tick(t.ms, cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                if *elem == ELEM {
                    return match self.face(cx) {
                        Face::Setup => {
                            self.activate_setup(cx, fx);
                            Handled::Yes
                        }
                        Face::Guide => self.key_guide(Key::Ok, 0, 0, cx.tick.ms, cx, fx),
                        Face::Loading => Handled::Yes,
                    };
                }
                self.activate_strip(*elem, fx)
            }
            ScreenEvent::PressCommit(_) => match cx.focus.current {
                Some(key) if key.elem >= plx_ui::dispatch::STRIP_BASE => self.activate_strip(key.elem, fx),
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
    fn groups(&self, _cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        out.push(GroupSpec {
            id: GROUP,
            kind: GroupKind::Free,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent: Rect::new(MARGIN_X, TOP, SCR_W - 2.0 * MARGIN_X, SCR_H - TOP - MARGIN_Y),
            len: 1,
            elem: ElemKind::Bare,
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
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "livetv row={} top={} setup_sel={} force_setup={} editing={}",
            self.cursor.row, self.cursor.top, self.setup_sel, self.force_setup, self.editing
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

// ---- drawing --------------------------------------------------------------------------------------

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

/// One line of text in `frame`, cut to fit with an ellipsis.
fn line(p: Painter, measure: &dyn plx_machine::machine::Measure, text: &str, sz: i32, col: [f32; 4], bold: bool, frame: Rect, h: HAlign) {
    if text.is_empty() || frame.w <= 0.0 {
        return;
    }
    let fitted = measure.fit_line(text, frame.w, sz, bold);
    let label = Label::new(fitted.as_ptr(), sz, col).h(h);
    if bold { label.bold().draw(p, frame) } else { label.draw(p, frame) };
}

impl LiveTvScreen {
    /// The rect of the guide cell under the cursor (or of the focused row's band when the row has
    /// nothing listed there) — what the engine places focus at.
    fn focus_cell_rect(&self, view: LiveTvView<'_>) -> Rect {
        let c = &self.cursor;
        let y = row_y(c.row, c.top);
        let window_end = c.window_ms + grid::WINDOW_MS;
        let cell = view
            .lineup()
            .channels
            .get(c.row)
            .and_then(|ch| ch.airing_at(c.at_ms).map(|i| &ch.airings[i]))
            .map(|a| (a.start_ms.max(c.window_ms), a.stop_ms.min(window_end)));
        match cell {
            Some((x0, x1)) => {
                let (l, r) = (x_of(x0, c.window_ms), x_of(x1, c.window_ms));
                Rect::new(l + CELL_GAP * 0.5, y, (r - l - CELL_GAP).max(1.0), ROW_H - ROW_GAP)
            }
            None => Rect::new(CELLS_X, y, CELLS_W, ROW_H - ROW_GAP),
        }
    }

    fn draw_guide(&self, p: Painter, view: LiveTvView<'_>, focused: bool, measure: &dyn plx_machine::machine::Measure) {
        let lineup = view.lineup();
        if lineup.is_empty() {
            StatusOverlay::new(guide_frame(), plx_platform::i18n::msg::livetv_empty_c(), StatusKind::Empty)
                .draw_measured(&plx_ui::Env::inert(), p, measure);
            return;
        }
        let c = self.cursor;
        let now = plx_base::wallclock::now_ms();
        self.draw_info(p, view, measure);
        // The time axis: a label every half hour.
        for k in 0..(grid::WINDOW_MS / grid::SLOT_MS) {
            let t = c.window_ms + k * grid::SLOT_MS;
            let x = x_of(t, c.window_ms);
            line(p, measure, &plx_ui::fmt::wall_time(t), theme::size::CAPTION, theme::TEXT_TERTIARY, false,
                Rect::new(x + theme::space::XS, AXIS_TOP, grid::SLOT_MS as f32 / grid::WINDOW_MS as f32 * CELLS_W - theme::space::XS, AXIS_H), HAlign::Left);
        }
        if !self.typed.is_empty() {
            line(p, measure, &self.typed, theme::size::HEADLINE, theme::TEXT_PRIMARY, true,
                Rect::new(MARGIN_X, AXIS_TOP, CH_W, AXIS_H), HAlign::Left);
        }
        let window_end = c.window_ms + grid::WINDOW_MS;
        let visible = visible_rows();
        for row in c.top..(c.top + visible).min(lineup.len()) {
            let ch = &lineup.channels[row];
            let y = row_y(row, c.top);
            let on_row = focused && row == c.row;
            let band = ROW_H - ROW_GAP;
            // The channel column.
            let chr = Rect::new(MARGIN_X, y, CH_W, band);
            let fill = if on_row { theme::GUIDE_CELL_NOW } else { theme::GUIDE_CELL };
            p.rect(chr, CELL_RAD, fill, fill, 0.0);
            let num_w = measure.width_str(&ch.number, theme::size::BODY, true) + theme::space::SM;
            line(p, measure, &ch.number, theme::size::BODY, theme::TEXT_PRIMARY, true,
                Rect::new(chr.x + theme::space::SM, y, num_w, band), HAlign::Left);
            line(p, measure, &ch.name, theme::size::LABEL, theme::TEXT_SECONDARY, false,
                Rect::new(chr.x + theme::space::SM + num_w, y, chr.w - 2.0 * theme::space::SM - num_w, band), HAlign::Left);
            // The airings in the window.
            let mut any = false;
            for a in ch.airings.iter().filter(|a| a.stop_ms > c.window_ms && a.start_ms < window_end) {
                any = true;
                let (l, r) = (x_of(a.start_ms.max(c.window_ms), c.window_ms), x_of(a.stop_ms.min(window_end), c.window_ms));
                let cell = Rect::new(l + CELL_GAP * 0.5, y, (r - l - CELL_GAP).max(1.0), band);
                let is_focus = on_row && a.covers(c.at_ms);
                let on_now = a.covers(now);
                let (fill, ink, sub) = if is_focus {
                    (theme::ACCENT, theme::ACCENT_INK, theme::ACCENT_INK)
                } else if on_now {
                    (theme::GUIDE_CELL_NOW, theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
                } else {
                    (theme::GUIDE_CELL, theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
                };
                p.rect(cell, CELL_RAD, fill, fill, 0.0);
                if on_now && !is_focus {
                    let elapsed = (x_of(now, c.window_ms) - cell.x).clamp(0.0, cell.w);
                    if elapsed > 0.0 {
                        p.rect(Rect::new(cell.x, cell.y, elapsed, cell.h), CELL_RAD, theme::GUIDE_CELL_ELAPSED, theme::GUIDE_CELL_ELAPSED, 0.0);
                    }
                }
                let inner = Rect::new(cell.x + theme::space::SM, cell.y + theme::space::XS, cell.w - 2.0 * theme::space::SM, cell.h * 0.5 - theme::space::XS);
                let title = if a.title.is_empty() { plx_platform::i18n::msg::livetv_no_info() } else { a.title.as_str() };
                line(p, measure, title, theme::size::LABEL, ink, true, inner, HAlign::Left);
                let times = format!("{} – {}", plx_ui::fmt::wall_time(a.start_ms), plx_ui::fmt::wall_time(a.stop_ms));
                line(p, measure, &times, theme::size::MICRO, sub, false,
                    Rect::new(inner.x, cell.y + cell.h * 0.5, inner.w, cell.h * 0.5 - theme::space::XS), HAlign::Left);
            }
            if !any {
                let cell = Rect::new(CELLS_X, y, CELLS_W, band);
                let (fill, ink) = if on_row { (theme::ACCENT, theme::ACCENT_INK) } else { (theme::GUIDE_CELL, theme::TEXT_TERTIARY) };
                p.rect(cell, CELL_RAD, fill, fill, 0.0);
                line(p, measure, plx_platform::i18n::msg::livetv_no_info(), theme::size::LABEL, ink, false,
                    Rect::new(cell.x + theme::space::SM, y, cell.w - 2.0 * theme::space::SM, band), HAlign::Left);
            }
        }
        // NOW.
        if now >= c.window_ms && now < window_end {
            let x = x_of(now, c.window_ms);
            let rows_h = (visible.min(lineup.len())) as f32 * ROW_H - ROW_GAP;
            p.rect(Rect::new(x - NOW_W * 0.5, AXIS_TOP + AXIS_H * 0.5, NOW_W, rows_h + AXIS_H * 0.5), NOW_W * 0.5, theme::GUIDE_NOW, theme::GUIDE_NOW, 0.0);
        }
    }

    /// The info pane: the focused airing's channel, title, facts and description.
    fn draw_info(&self, p: Painter, view: LiveTvView<'_>, measure: &dyn plx_machine::machine::Measure) {
        let c = self.cursor;
        let Some(ch) = view.lineup().channels.get(c.row) else { return };
        let airing = ch.airing_at(c.at_ms).map(|i| &ch.airings[i]);
        let x = MARGIN_X;
        let mut y = TOP;
        let channel = format!("{} \u{b7} {}", ch.number, ch.name);
        let cap = measure.line_h(theme::size::CAPTION);
        line(p, measure, &channel, theme::size::CAPTION, theme::TEXT_SECONDARY, false, Rect::new(x, y, INFO_W, cap), HAlign::Left);
        y += cap + theme::space::XS;
        let title_h = measure.line_h(theme::size::TITLE);
        let title = airing.map(|a| a.title.as_str()).filter(|t| !t.is_empty()).unwrap_or(plx_platform::i18n::msg::livetv_no_info());
        line(p, measure, title, theme::size::TITLE, theme::TEXT_PRIMARY, true, Rect::new(x, y, INFO_W, title_h), HAlign::Left);
        y += title_h + theme::space::XS;
        let Some(a) = airing else {
            if let Some(e) = view.guide_error() {
                let _ = e;
                let body_h = measure.line_h(theme::size::BODY);
                line(p, measure, plx_platform::i18n::msg::livetv_guide_missing(), theme::size::BODY, theme::TEXT_SECONDARY, false,
                    Rect::new(x, y, INFO_W, body_h), HAlign::Left);
            }
            return;
        };
        let mut facts = vec![format!("{} – {}", plx_ui::fmt::wall_time(a.start_ms), plx_ui::fmt::wall_time(a.stop_ms))];
        for part in [&a.episode, &a.sub_title, &a.category] {
            if !part.is_empty() {
                facts.push(part.clone());
            }
        }
        let body_h = measure.line_h(theme::size::BODY);
        line(p, measure, &facts.join(" \u{b7} "), theme::size::BODY, theme::TEXT_SECONDARY, false, Rect::new(x, y, INFO_W, body_h), HAlign::Left);
        y += body_h + theme::space::XS;
        if !a.desc.is_empty() {
            let view = TextView::new(&a.desc, theme::size::LABEL, theme::TEXT_READING).with_measure(measure).max_lines(2);
            let h = view.measure_h(INFO_W).min(AXIS_TOP - y - theme::space::XS);
            view.draw(p, Rect::new(x, y, INFO_W, h.max(0.0)));
        }
    }

    fn draw_setup(&self, p: Painter, view: LiveTvView<'_>, focused: bool, tick_ms: u32, measure: &dyn plx_machine::machine::Measure) {
        let x = MARGIN_X;
        let mut y = TOP + theme::space::MD;
        let title_h = measure.line_h(theme::size::TITLE);
        line(p, measure, plx_platform::i18n::msg::livetv_setup_title(), theme::size::TITLE, theme::TEXT_PRIMARY, true,
            Rect::new(x, y, INFO_W, title_h), HAlign::Left);
        y += title_h + theme::space::SM;
        let body = TextView::new(plx_platform::i18n::msg::livetv_setup_body(), theme::size::BODY, theme::TEXT_READING).with_measure(measure).max_lines(3);
        let body_h = body.measure_h(1_100.0);
        body.draw(p, Rect::new(x, y, 1_100.0, body_h));
        y += body_h + theme::space::SM;
        let note: Option<String> = if let Status::Failed(_) = view.status() {
            Some(plx_platform::i18n::msg::livetv_setup_failed(view.source()))
        } else if matches!(view.discovery(), Discovery::Done(found) if found.is_empty()) {
            Some(plx_platform::i18n::msg::livetv_setup_none_found().to_owned())
        } else {
            None
        };
        if let Some(note) = note {
            line(p, measure, &note, theme::size::BODY, theme::TEXT_SECONDARY, false,
                Rect::new(x, y, INFO_W, measure.line_h(theme::size::BODY)), HAlign::Left);
        }
        let items = setup_items(view, self.force_setup);
        let sel = self.setup_sel.min(items.len().saturating_sub(1));
        for (i, item) in items.iter().enumerate() {
            let r = setup_row_rect(i);
            let on = focused && i == sel;
            match item {
                Item::Address => self.draw_field(p, r, on, tick_ms, measure),
                _ => {
                    let text: String = match item {
                        Item::Search if matches!(view.discovery(), Discovery::Searching) => plx_platform::i18n::msg::livetv_setup_searching().to_owned(),
                        Item::Search => plx_platform::i18n::msg::livetv_setup_search().to_owned(),
                        Item::Found(k) => match view.discovery() {
                            Discovery::Done(found) => found.get(*k).map(|f| format!("{} \u{b7} {}", f.name, f.origin)).unwrap_or_default(),
                            _ => String::new(),
                        },
                        Item::Retry => plx_platform::i18n::msg::livetv_setup_retry().to_owned(),
                        Item::Back => plx_platform::i18n::msg::livetv_setup_back().to_owned(),
                        Item::TurnOff => plx_platform::i18n::msg::livetv_setup_turn_off().to_owned(),
                        Item::Address => unreachable!(),
                    };
                    let fitted = measure.fit_line(&text, FIELD_W - 2.0 * FIELD_PAD, theme::size::BODY, true);
                    let w = (measure.width(&fitted, theme::size::BODY, true) + 44.0).min(FIELD_W);
                    TabPill::new(fitted.as_ptr(), theme::size::BODY, Rect::new(r.x, r.y, w, r.h))
                        .focused(on)
                        .draw(&plx_ui::Env::inert(), p);
                }
            }
        }
    }

    /// The address field: the typed text and a caret while the keyboard is up, the hint while it
    /// is empty.
    fn draw_field(&self, p: Painter, r: Rect, on: bool, tick_ms: u32, measure: &dyn plx_machine::machine::Measure) {
        let fill = if on { theme::CONTROL_IDLE_FILL } else { theme::CONTROL_IDLE_FILL_UNKEYED };
        p.rect(r, r.h * 0.5, fill, fill, 0.0);
        if on {
            p.rring(r, r.h * 0.5, 2.0, theme::CONTROL_RIM_FOCUS_UNKEYED);
        }
        let inner = Rect::new(r.x + FIELD_PAD, r.y, r.w - 2.0 * FIELD_PAD, r.h);
        let text = self.address.text();
        if text.is_empty() && !self.editing {
            line(p, measure, plx_platform::i18n::msg::livetv_setup_address(), theme::size::BODY, theme::CONTROL_IDLE_INK, true, inner, HAlign::Left);
            return;
        }
        if text.is_empty() {
            line(p, measure, plx_platform::i18n::msg::livetv_setup_address_hint(), theme::size::BODY, theme::TEXT_TERTIARY, false, inner, HAlign::Left);
        } else {
            line(p, measure, text, theme::size::BODY, theme::FIELD_EDITING_INK, false, inner, HAlign::Left);
        }
        if self.editing && (tick_ms / 530) % 2 == 0 {
            let before = &text[..self.address.caret().min(text.len())];
            let cx = inner.x + measure.width_str(before, theme::size::BODY, false);
            let cap = measure.cap_h(theme::size::BODY);
            p.rect(Rect::new(cx.min(inner.x + inner.w), r.y + (r.h - cap * 1.4) * 0.5, 2.0, cap * 1.4), 1.0,
                theme::FIELD_EDITING_INK, theme::FIELD_EDITING_INK, 0.0);
        }
    }
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
    }
}
