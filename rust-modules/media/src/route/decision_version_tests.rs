//! The version picker's route half: which part and codecs a picked version plays, and the version
//! list the player's Version page reads off the publication.
use super::*;
use plx_data::metadata::{MediaVersion, PlayingItem};

fn versions() -> Vec<MediaVersion> {
    vec![
        MediaVersion { vcodec: "hevc".into(), acodec: "eac3".into(), part: "/library/parts/1/a.mkv".into(), ..Default::default() },
        MediaVersion { vcodec: "h264".into(), acodec: "aac".into(), part: "/library/parts/2/b.mp4".into(), ..Default::default() },
    ]
}

/// **A picked version plays its OWN part and codecs** (the direct-play URL and the Load payload's
/// codec pair); version 0 keeps the caller's fields, and a version with no part is not playable.
#[test]
fn a_picked_version_plays_its_own_part() {
    let mut item = PlayingItem::with_subs(Vec::new());
    item.versions = versions();
    item.media_index = 1;
    assert_eq!(
        version_play_fields(&item),
        Some(("/library/parts/2/b.mp4".to_string(), "h264".to_string(), "aac".to_string()))
    );
    item.media_index = 0;
    assert_eq!(version_play_fields(&item), None, "version 0 is what the caller already named");
    item.media_index = 1;
    item.versions[1].part.clear();
    assert_eq!(version_play_fields(&item), None);
}

/// The version list and the playing index reach the screens' copy, shared rather than copied.
#[test]
fn the_publication_carries_the_versions() {
    let mut ps = PlaybackSession::IDLE;
    with_versions(&ps, |v, i| assert!(v.is_empty() && i == 0));
    install_versions_for_test(&mut ps, versions(), 1);
    let shown = ps.publication();
    with_versions(&shown, |v, i| {
        assert_eq!((v.len(), i), (2, 1));
        with_versions(&ps, |held, _| assert_eq!(held.as_ptr(), v.as_ptr()));
    });
}
