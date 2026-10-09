//! The screensaver-delay persistence seam: durable-first, live only once the write lands.
use super::*;
use plx_plex::plex::session::Screensaver;

struct Restore(Screensaver);
impl Drop for Restore {
    fn drop(&mut self) {
        restore_screensaver(self.0);
    }
}

/// **A durable pick is live and read back by the next load**; the default is five minutes.
#[test]
fn a_durable_pick_is_live_and_persisted() {
    let _g = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("screensaver-set");
    let _restore = Restore(screensaver());
    restore_screensaver(Screensaver::default());
    assert_eq!(screensaver(), Screensaver::Minutes5, "the shipped default");

    assert!(set_screensaver(Screensaver::Off));
    assert_eq!(screensaver(), Screensaver::Off);
    assert_eq!(plx_plex::plex::session::load().screensaver(), Screensaver::Off);

    assert!(set_screensaver(Screensaver::Minutes5));
    assert_eq!(plx_plex::plex::session::load().screensaver(), Screensaver::Minutes5);
}
