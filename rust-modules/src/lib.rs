//! PlxNative — an unofficial native Plex client for LG webOS.
//! Copyright © 2026 Gleb Linnik. Licensed under GPL-3.0-or-later; see LICENSE at the repository
//! root, and THIRD-PARTY-NOTICES.md for the components this links or redistributes.
//! Not affiliated with, endorsed by, or sponsored by Plex GmbH or LG Electronics.
//!
//! plxnative-modules — the Rust app core, built as a staticlib and linked into the C
//! boot shim. The crate's C surface is tiny: C calls `plex_run` (port.rs), writes the fallback
//! image marker through `plx_crash_write_image_marker`, re-enters the native-crash spool through
//! `plx_sentry_spool_external`, and forwards the two Starfish callbacks (`sf_on_event`/
//! `acb_on_event`, `plx_media`'s player/mod.rs). Everything else is Rust-internal (the per-module `repr(C)`
//! shapes are migration legacy, not ABI).
mod app; // run_application — the Rust app core / event loop (the entry inverted from main.c; port.rs's `plex_run` hands over to it)
mod capture; // dev live UI capture stream: own-GLES-frame grab → MPEG1/TS or JPEG → TCP (UI plane only)
mod coldstart; // retires old last-page bookmarks; authenticated cold boots now stay on Home
mod dev; // the /tmp/plxnative-* trigger surface, behind one `devtriggers` feature — read it before adding a trigger
mod focusprobe; // dev: one diffable line naming everything app.rs's key ladder can move, logged when it changes
mod lab; // Cloud Lab bridge: pinned diagnostic uploads + optional outbound command long-poll
// Pure RELEASE_LINE-parsing helpers, `include!`d verbatim by build.rs so `cargo test --lib`
// actually runs their unit tests (see the module for why). Nothing in the app itself calls
// them at runtime — the version rule they implement is applied once, at compile time, by
// build.rs — so they exist in THIS crate only for the test build; `#[cfg(test)]` here, not on
// the functions themselves, because build.rs's own separate compilation is never built with
// `--test` and needs them unconditionally.
#[cfg(test)]
mod release_line;
mod remote; // dev/testing remote-control channel: a FIFO the loop drains into synthetic SDL keys
#[cfg(feature = "hostsim")]
mod shot; // simulator screenshots: read the frame back and write a PNG (see the module doc)
#[cfg(not(feature = "hostsim"))]
mod system; // the webOS port's SDL/Wayland window glue; the desktop port has its own

mod textinput; // the TV's own on-screen keyboard, via plain SDL_StartTextInput (see the module doc)

/// The instance root, for the simulator binary.
///
/// `src/bin/sim.rs` is a separate crate and cannot see `pub(crate)` items, but it must create the
/// directory and truncate the event log inside it before the app starts. Exposing the resolver
/// keeps ONE definition of where that is — a second `env::var` read in the binary would be a
/// second answer waiting to drift from this one.
#[cfg(feature = "hostsim")]
pub fn sim_runtime_dir() -> std::path::PathBuf {
    plx_base::paths::runtime_dir().to_path_buf()
}

/// The event log's path, built by the ONE expression [`plx_base::eventlog::log`] uses.
///
/// `src/bin/sim.rs` truncates this file at startup. Spelling the name a second time over there
/// would mean a rename could leave the binary truncating a file the app never appends to — the
/// simulator's log would silently start non-empty, which is exactly the state `tests/run.py` dates
/// its first line from.
#[cfg(feature = "hostsim")]
pub fn sim_events_log() -> std::path::PathBuf {
    plx_base::eventlog::events_log()
}

/// The two ports, one per build: `port` is the webOS port the C shim (`src/main.c`) enters through,
/// and `desktop` is the simulator's (and the desktop app's) own port, which `src/bin/sim.rs` enters
/// through. Each holds a `plex_run` that installs its table and hands over to the same
/// `app::run_application`. The simulator reaches its entry by path (`plxnative_modules::desktop::
/// plex_run`) so the compiler checks the signature. There is deliberately no `pub use` here: a
/// re-export would be a reference from the crate root into a port.
#[cfg(not(feature = "hostsim"))]
mod port;
#[cfg(feature = "hostsim")]
pub mod desktop;
#[cfg(feature = "hostsim")]
pub use app::synthetic_home_initial;

