//! Physically owned Live TV model (`crate::livetv`) and its machine.
//!
//! The state's workers are plain threads answering through a channel, not `tape`-admitted content
//! resources: nothing a recording replays reaches Live TV, and under an active tape the state
//! refuses to start a worker at all (`LiveTvState::start_load`), so a replay stays deterministic.

use crate::livetv::{LiveTvCmd, LiveTvState, LiveTvView};
use plx_machine::machine::{Cx, Effects, Handled, Host, Machine};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::StoreEv;

#[derive(Default)]
pub struct LiveTvStore {
    state: LiveTvState,
    notice_gen: AtomicU32,
    notice_dirty: AtomicBool,
}

impl LiveTvStore {
    fn bump(&self) {
        self.notice_dirty.store(true, Ordering::Relaxed);
        self.notice_gen.fetch_add(1, Ordering::Relaxed);
    }
    pub fn gen(&self) -> u32 {
        self.notice_gen.load(Ordering::Relaxed)
    }
    pub fn take_notice(&self) -> Option<u32> {
        self.notice_dirty.swap(false, Ordering::Relaxed).then(|| self.gen())
    }
    pub fn view(&self) -> LiveTvView<'_> {
        self.state.view()
    }
    pub fn run(&mut self, cmd: LiveTvCmd) -> bool {
        let changed = self.state.run(cmd, plx_base::wallclock::now_ms());
        if changed {
            self.bump();
        }
        changed
    }
    pub fn pump(&mut self) -> bool {
        let changed = self.state.pump(plx_base::wallclock::now_ms());
        if changed {
            self.bump();
        }
        changed
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn install_for_test(&mut self, origin: &str, lineup: crate::livetv::guide::Lineup) {
        self.state.install_for_test(origin, lineup, plx_base::wallclock::now_ms());
        self.bump();
    }
}

impl<H: Host> Machine<H> for LiveTvStore {
    type Ev = StoreEv<LiveTvCmd>;
    fn step(&mut self, ev: &Self::Ev, _cx: &Cx<'_, H>, _fx: &mut Effects<'_, H>) -> Handled {
        match ev {
            StoreEv::Cmd(cmd) => {
                self.run(cmd.clone());
            }
            StoreEv::Pump { .. } => {
                self.pump();
            }
        }
        Handled::Yes
    }
}
