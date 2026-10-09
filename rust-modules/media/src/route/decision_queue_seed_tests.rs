//! Playing from an EXISTING PlayQueue: the shuffle landing, the seed a successor continues in, and
//! the queue a continuation installs. Pure halves only — the GET and the POST are the plex crate's
//! (`timeline.rs` grades their wire shape and the centred window).
use super::*;
use plx_plex::plex::QueueRow;

const S: ServerId = ServerId::from_raw(0);

fn ep(item_id: i64, rk: &str, season: i64, index: i64) -> QueueRow {
    QueueRow {
        item_id,
        sid: S,
        rk: rk.into(),
        kind: "episode".into(),
        title: format!("t{rk}"),
        show_title: "show".into(),
        season,
        index,
        part: format!("/library/parts/{rk}/file.mkv"),
        ..Default::default()
    }
}

/// A shuffled queue as the server answers it: episodes out of order, the second row selected.
fn shuffled() -> Vec<QueueRow> {
    vec![ep(11, "305", 3, 5), ep(12, "102", 1, 2), ep(13, "207", 2, 7), ep(14, "101", 1, 1)]
}

/// **A shuffle starts on the server's pick and keeps its queue.** The landing's first row is the
/// selected one (not the queue's first row), and its seed is sticky — so the playback after it
/// continues in the shuffle — with the POST's rows already in hand (no second round trip).
#[test]
fn a_shuffle_lands_on_the_servers_pick_with_a_sticky_seed() {
    let landing = shuffle_landing_of(S, 41, 12, shuffled()).expect("a shuffle with rows lands");
    assert_eq!(landing.first.rk, "102");
    assert_eq!(landing.first.item_id, 12);
    assert_eq!((landing.seed.id, landing.seed.item_id), (41, 12));
    assert!(landing.seed.sticky && !landing.seed.refetch);
    assert_eq!(landing.seed.rows.len(), 4);

    let unnamed = shuffle_landing_of(S, 41, 0, shuffled()).unwrap();
    assert_eq!(unnamed.first.rk, "305", "no selection named: the queue's head");
    assert!(shuffle_landing_of(S, 41, 12, Vec::new()).is_none(), "an empty shuffle starts nothing");
}

/// **A continuation's successor is the QUEUE's next row**, not the next episode in order: after
/// S1E2 in this shuffle comes S2E7. The ids go on the timeline as the seed names them, and the
/// stickiness rides through to the session.
#[test]
fn a_continuation_names_the_shuffled_successor() {
    let seed = QueueSeed { id: 41, item_id: 12, rows: shuffled(), refetch: false, sticky: true };
    let q = queue_info_from_seed(&seed, seed.rows.clone(), S, "102");
    assert_eq!((q.id.as_str(), q.item_id.as_str()), ("41", "12"));
    let next = q.up_next.expect("a successor");
    assert_eq!((next.rk.as_str(), next.season, next.index, next.item_id), ("207", 2, 7, 13));
    assert!(q.sticky);
    assert!(q.machine_id.is_empty(), "a queue that exists needs no /identity");

    let last = QueueSeed { item_id: 14, ..seed };
    assert!(queue_info_from_seed(&last, last.rows.clone(), S, "101").up_next.is_none(), "the end of the shuffle");
}

/// **Up Next continues in a sticky queue and ONLY a sticky one.** A shuffled playback's successor
/// is seeded with the held rows (the fallback) and a refetch; an ordinary playback's is not
/// seeded at all, so it POSTs a fresh `continuous=1` queue exactly as before shuffle existed. A
/// playback with no queue id is never seeded, sticky or not.
#[test]
fn up_next_is_seeded_only_from_a_sticky_queue() {
    let mut ps = PlaybackSession::IDLE;
    ps.pq_id = "41".into();
    ps.pq_item_id = "12".into();
    ps.queue = Some(std::sync::Arc::new(shuffled()));
    let next = UpNext { rk: "207".into(), item_id: 13, ..Default::default() };

    assert!(up_next_seed(&ps, &next).is_none(), "an ordinary queue: a fresh one for the successor");

    ps.queue_sticky = true;
    let seed = up_next_seed(&ps, &next).expect("a shuffle's successor stays in it");
    assert_eq!((seed.id, seed.item_id), (41, 13));
    assert!(seed.refetch && seed.sticky);
    assert_eq!(seed.rows, shuffled());

    ps.pq_id.clear();
    assert!(up_next_seed(&ps, &next).is_none(), "no queue id, nothing to continue in");
}

/// **A queue-panel jump continues in the queue whatever its stickiness**, and carries that
/// stickiness on unchanged: a jump inside an ordinary queue stays ordinary afterwards.
#[test]
fn a_jump_continues_in_the_queue_and_keeps_its_stickiness() {
    let mut ps = PlaybackSession::IDLE;
    ps.pq_id = "7".into();
    ps.queue = Some(std::sync::Arc::new(shuffled()));
    let seed = queue_seed_for(&ps, 14).expect("a queue id: a seed");
    assert_eq!((seed.id, seed.item_id, seed.sticky), (7, 14, false));
    ps.queue_sticky = true;
    assert!(queue_seed_for(&ps, 14).unwrap().sticky);
}

/// **The queue reaches the screens' copy, shared.** The Play queue page reads the rows off the
/// per-frame publication, so it must carry them — by reference, not as a copy of every row.
#[test]
fn the_publication_shares_the_queue() {
    let mut ps = PlaybackSession::IDLE;
    ps.queue = Some(std::sync::Arc::new(shuffled()));
    ps.queue_sticky = true;
    let a = ps.publication();
    with_queue(&a, |rows| assert_eq!(rows.len(), 4));
    with_queue(&ps, |held| with_queue(&a, |shown| assert_eq!(held.as_ptr(), shown.as_ptr())));
    assert!(queue_is_sticky(&a));
    with_queue(&PlaybackSession::IDLE, |rows| assert!(rows.is_empty()));
}

/// **The panel's descriptor starts any row** — a movie included, which Up Next's episodes-only
/// gate refuses — and carries the row's queue identity.
#[test]
fn any_queue_row_has_a_descriptor() {
    let movie = QueueRow { item_id: 3, rk: "9".into(), kind: "movie".into(), ..Default::default() };
    assert!(up_next_of(&movie).is_none());
    let d = queue_row_descriptor(&movie).expect("the panel can start a movie row");
    assert_eq!((d.rk.as_str(), d.item_id), ("9", 3));
    assert!(queue_row_descriptor(&QueueRow::default()).is_none());
}
