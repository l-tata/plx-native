//! A Live TV channel installed as the playback (`install_live_stream`) after a Plex film.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;
use super::test_support::apply_plan;

/// **A channel is never on Auto's Original watch.** The watch belongs to a Plex direct play that
/// Auto may move onto an HLS transcode; a channel has no Plex item to transcode. Installing one
/// right after such a film used to leave the film's contract, rate and watch flag in place, so
/// the demuxer would watch the channel and could ask for a transcode of nothing.
#[test]
fn a_channel_after_a_watched_original_film_is_not_watched() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    restore_quality(Quality::Auto);
    // The film: an Auto HLS route that recovered to its Original — the state the watch runs in.
    apply_plan(
        &mut ps,
        Plan {
            url: "https://example.invalid/hls/master.m3u8".into(),
            tsession: "encoder-1".into(),
            vcodec: "h264".into(),
            acodec: "aac".into(),
            src_vcodec: "hevc".into(),
            src_acodec: "eac3".into(),
            contract: plx_plex::plex::EncodeContract {
                delivery: plx_plex::plex::TranscodeDelivery::FixedHls { seconds_per_segment: 2 },
                ceiling: Some(crate::abr::Rung::P1080High.ceiling()),
                ..Default::default()
            },
            transport_kbps: 28_000,
            auto_original: Some(test_original_candidate(None)),
            ..Default::default()
        },
        "rk-film",
    );
    assert_eq!(recover_auto_to_original(&mut ps, 120), Some(AutoOriginalReload::Direct));
    assert!(auto_original_watch(&ps).is_some(), "the film is on the watch");

    let lineup = std::sync::Arc::new(plx_data::livetv::guide::Lineup::default());
    let mut session = crate::live::LiveSession::tuning(lineup, 0, None);
    session.facts = Some(crate::live::StreamFacts::TUNARR_DEFAULT);
    assert!(install_live_stream(&mut ps, session, "http://192.0.2.20:8000/stream/channels/1.ts"));
    assert!(auto_original_watch(&ps).is_none(), "the channel is not");
    restore_quality(Quality::Original);
    reset_session(&mut ps);
    install_active_encoder("");
}
