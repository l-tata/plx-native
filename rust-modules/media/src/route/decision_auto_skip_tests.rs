//! The auto-skip (intro / credits) persistence seam: durable-first, live only once the write lands.
use super::*;
use plx_plex::plex::session::AutoSkip;

struct Restore(AutoSkip);
impl Drop for Restore {
    fn drop(&mut self) {
        restore_auto_skip(self.0);
    }
}

/// **A durable pick is live and read back by the next load**; the default is Off.
#[test]
fn a_durable_pick_is_live_and_persisted() {
    let _g = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("auto-skip-set");
    let _restore = Restore(auto_skip());
    restore_auto_skip(AutoSkip::Off);
    assert_eq!(auto_skip(), AutoSkip::Off, "the shipped default skips nothing");

    assert!(set_auto_skip(AutoSkip::Intro));
    assert_eq!(auto_skip(), AutoSkip::Intro);
    assert_eq!(plx_plex::plex::session::load().auto_skip(), AutoSkip::Intro);

    // choosing the default again removes the key from the file
    assert!(set_auto_skip(AutoSkip::Off));
    assert_eq!(plx_plex::plex::session::load().auto_skip(), AutoSkip::Off);
}

/// **A failed write claims nothing**: the live value stays.
#[test]
fn a_failed_write_changes_nothing() {
    let _g = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("auto-skip-failed");
    let _restore = Restore(auto_skip());
    restore_auto_skip(AutoSkip::Off);
    // the session file's parent is a regular file, so no write can land
    let dir = std::env::temp_dir().join(format!("plxnative-auto-skip-blocked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("blocker"), b"x").unwrap();
    plx_plex::plex::session::redirect_for_test(Some(dir.join("blocker").join("auth.json")));

    let saved = set_auto_skip(AutoSkip::Intro);
    plx_plex::plex::session::redirect_for_test(None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!saved);
    assert_eq!(auto_skip(), AutoSkip::Off);
}
