//! The desktop port: the table the simulator (and, as it grows, the desktop app) installs in place
//! of the webOS one, and the entry that installs it (step L15b, docs/module-layers.md).
//!
//! It is a second implementation of the `plx_platform::tv` interfaces, not the webOS table running
//! its host arms: nothing here names `webos`, `keymanager`, `system` or `player::ffi`, which are
//! not compiled into this build. Each entry answers what the simulator answered when it ran the
//! webOS table, minus the lines that only made sense on a television (`webos: /etc/... unreadable`,
//! `devjail: soc=`). The answers that a real desktop app needs instead (a key store, the OS locale
//! and clock, its own window behaviour) belong here as they are written.
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Once;

use plx_base::eventlog::log;
use plx_platform::tv::{self, sandbox, secure, toast};

static PORT: tv::Port = tv::Port {
    probe_device,
    start_capability_probe,
    repair_sandbox,
    seal,
    open: |_| None,
    remove: |_, _| {},
    system_locale: || tv::LocaleReply::NoPlatform,
    system_utc_offset_s: || None,
    go_home,
    poll_home: nothing,
    deliver_toast,
    bind_window: |_| {},
    grab_surface,
    release_surface: nothing,
    arm_opaque_region: nothing,
    opaque_route: |_| {},
    clear_opaque_region: nothing,
    pump_bus: nothing,
    frame_probe_request: nothing,
    frame_probe_waiting: nothing,
    frame_probe_acquired: nothing,
    frame_probe_fields: String::new,
    sink: &plx_media::player::ffi_host::HostSink,
};

/// The simulator's entry (`src/bin/sim.rs`): install the port, then hand over to
/// `app::run_application`, the same body the webOS port's `plex_run` hands over to.
pub extern "C" fn plex_run(pms_host: *const c_char, pms_port: c_int) -> c_int {
    let _ = tv::install(&PORT);
    crate::app::run_application(pms_host, pms_port)
}

fn nothing() {}

/// Which desktop this is, for the log. The device identity `tv::device` carries is a television's
/// (firmware release, codename, model, board), so it stays empty here, which every reader already
/// prints as unknown rather than inventing a value. A desktop has no jail between the app and its
/// video path, so the sandbox verdict is the one a television outside the affected family gets.
fn probe_device() {
    log(&format!(
        "desktop: os={} arch={} family={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY
    ));
    tv::device::publish_info(tv::device::Info::default());
    tv::device::publish_hardware(tv::device::Hardware::default());
    sandbox::publish(sandbox::Verdict::NotApplicable);
}

/// The Dolby Vision answer: the developer override when one is armed, else unknown from the host,
/// which is what the simulator has always published. Asking the display and decoders is the
/// desktop playback engine's job.
fn start_capability_probe() {
    use plx_platform::devcaps::dv::{self, DvCapability, DvProbe, ProbeSource};
    let probe = match dv::forced() {
        Some((capability, conflict)) => {
            if conflict {
                log("desktop-caps: dvcaps0 and dvcaps1 both armed; dvcaps0 wins");
            }
            let reason = if conflict { "override-conflict" } else { "override" };
            DvProbe { capability, source: ProbeSource::Override, reason }
        }
        None => DvProbe { capability: DvCapability::Unknown, source: ProbeSource::Host, reason: "host" },
    };
    dv::publish(probe);
    log(&format!(
        "desktop-caps: dolby-vision answer={} source={}",
        probe.capability.label(),
        probe.provenance()
    ));
    plx_machine::idle::invalidate();
}

/// There is no jail to repair. Never offered in practice: the verdict above never blocks.
fn repair_sandbox() -> Result<(), sandbox::Failure> {
    Err(sandbox::Failure::Unsupported)
}

/// No desktop key store yet, so the session falls back to the mode-0600 file, as it did when the
/// simulator ran the webOS key manager with no bus to reach it. Said once per process.
fn seal(_plain: &[u8]) -> Option<secure::Sealed> {
    static SAID: Once = Once::new();
    SAID.call_once(|| log("session protection: no desktop key store; using the 0600 file fallback"));
    None
}

/// BACK at a root: a desktop has no launcher to hand the screen to, so the press stays in the app.
fn go_home() {
    log("gohome: no system launcher on the desktop — the root press stays in the app");
}

fn deliver_toast(_message: &str, identity: toast::Identity) -> toast::Sent {
    log(&format!("toast: no system toast on the desktop — {identity:?} toast not sent"));
    toast::Sent { reply: None, outcome: toast::Outcome::NoBus }
}

/// What the window presents through. A desktop SDL window has no Wayland surface to borrow and no
/// hardware video plane beneath it, so this only reports what the driver granted.
fn grab_surface(_win: *mut c_void) {
    extern "C" {
        fn SDL_GetVersion(ver: *mut u8);
    }
    let mut version = [0u8; 3];
    unsafe { SDL_GetVersion(version.as_mut_ptr()) };
    let alpha = plx_gfx::gfx::log_framebuffer_bits();
    log(&format!("wm sdl={}.{}.{} desktop alpha={alpha}", version[0], version[1], version[2]));
}
