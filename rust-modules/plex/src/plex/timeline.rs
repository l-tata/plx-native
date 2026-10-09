//! Playback-session protocol ops (impl Client): the `/:/timeline` progress report and the
//! two calls that make the session a first-class, remote-controllable player — GET /identity
//! (the server id the PlayQueue uri needs) and POST /playQueues.
//!
//! Rebuilt FROM the live `route.rs`/`threads.rs` (task #26): the timeline carries the
//! per-playback session id, PlayQueue ids, and the SELECTED audio/subtitle stream ids, so
//! /status/sessions shows the right track and the Direct Play vs Transcode badge
//! (correlated by the active encoder's coupled `X-Plex-Session-Identifier == session=`).
use super::client::{Client, QueryBuilder};
use super::params::TimelineReport;
use super::servers::ServerId;

impl Client {
    /// POST /:/timeline (the spec verb; params ride the query) — the server updates viewOffset
    /// (the resume point) + watched state. `true` when it took the report.
    ///
    /// The outcome used to stop here (`post_void`), which made this the one write in the playback
    /// protocol that could fail in complete silence: a 401 on a revoked token, a refused connect
    /// and a 500 were indistinguishable from a committed resume point, and the caller logged the
    /// same success line for all four. The reporting is `crate::route::scrobble_stop`'s; this
    /// only has to stop throwing the answer away.
    pub fn timeline(&self, r: &TimelineReport) -> bool {
        let q = QueryBuilder::new("/:/timeline")
            .str("ratingKey", r.rating_key)
            .str("key", &format!("/library/metadata/{}", r.rating_key))
            .str("identifier", "com.plexapp.plugins.library")
            .str("state", r.state.as_str())
            .int("time", r.time_ms)
            .int("duration", r.duration_ms)
            .str("X-Plex-Session-Identifier", r.session);
        let q = self
            .playback_identity(q)
            .opt_str("playQueueID", r.play_queue_id)
            .opt_str("playQueueItemID", r.play_queue_item_id)
            .opt_int("audioStreamID", r.audio_stream_id)
            .opt_int("subtitleStreamID", r.subtitle_stream_id);
        self.post_ok(&q.build())
    }

    /// GET /identity → the server's stable machineIdentifier (None on failure/empty).
    pub fn machine_identity(&self) -> Option<String> {
        let mid = self.get_json("/identity")?.machine_identifier;
        if mid.is_empty() {
            None
        } else {
            Some(mid)
        }
    }

    /// POST /playQueues for one item. Best-effort: None on failure — the timeline still works,
    /// just without the queue ids (and the player without an Up Next).
    ///
    /// `continuous=1` is what makes the response carry the show's remaining episodes after the
    /// one being played, each as a FULL `Metadata` row (thumb, S/E, duration, viewOffset, and
    /// `Media[0].Part[0].key` + codecs — everything `route::request_play` needs). The Up Next control
    /// screen is therefore free: it reads [`PlayQueueResult::next`] from the queue this playback
    /// already had to create, instead of asking the server what plays next.
    ///
    /// Trailer extras omit `continuous`: a continuous extras queue can be sibling clips, and EOS
    /// must not Up-Next into a featurette. `opt_int` drops the param when `continuous` is false.
    ///
    /// The WHOLE window is kept, as [`QueueRow`]s — the queue this round trip already paid for is
    /// the queue a list can draw and jump around in, and throwing it away meant re-asking the
    /// server for something it had already sent.
    pub fn create_play_queue(
        &self,
        machine_id: &str,
        rating_key: &str,
        session: &str,
        continuous: bool,
    ) -> Option<PlayQueueResult> {
        self.post_play_queue(machine_id, rating_key, session, continuous, false)
    }

    /// POST /playQueues with `shuffle=1` over a SHOW or a SEASON (`container_key`): the server
    /// shuffles every episode under it and selects the first one of its shuffle. Not
    /// `continuous` — a shuffled queue already holds everything it will ever play.
    ///
    /// The result's [`PlayQueueResult::items`] is that queue's window and its selected row is what
    /// plays first; the caller then keeps PLAYING FROM THIS QUEUE (`route::QueueSeed`), because a
    /// fresh `continuous=1` queue for the next episode would be the show in order again.
    pub fn create_shuffle_queue(
        &self,
        machine_id: &str,
        container_key: &str,
        session: &str,
    ) -> Option<PlayQueueResult> {
        self.post_play_queue(machine_id, container_key, session, false, true)
    }

    fn post_play_queue(
        &self,
        machine_id: &str,
        rating_key: &str,
        session: &str,
        continuous: bool,
        shuffle: bool,
    ) -> Option<PlayQueueResult> {
        let uri = format!(
            "server://{machine_id}/com.plexapp.plugins.library/library/metadata/{rating_key}"
        );
        let q = play_queue_query(&uri, continuous, shuffle)
            .str("X-Plex-Session-Identifier", session);
        let q = self.playback_identity(q);
        // a shuffled queue's selection is the server's pick, never the container we named — the
        // rating-key fallback must not match the container
        let fallback_rk = if shuffle { "" } else { rating_key };
        Some(PlayQueueResult::of(
            self.post_json(&q.build())?,
            self.id(),
            fallback_rk,
        ))
    }

    /// GET /playQueues/{id}: the window of an EXISTING queue centred on `center_item_id`, for a
    /// playback that continues inside the queue it is already playing (a shuffled queue's next
    /// episode, a row jumped to from the queue panel). `center` does not move the server's own
    /// selection (spec: "this doesn't change the current selected item") — the timeline report's
    /// `playQueueItemID` does — so the result's selection is `center_item_id`, not the container's.
    pub fn fetch_play_queue(
        &self,
        play_queue_id: i64,
        center_item_id: i64,
        rating_key: &str,
    ) -> Option<PlayQueueResult> {
        let q = fetch_queue_query(play_queue_id, center_item_id);
        let q = self.playback_identity(q);
        Some(PlayQueueResult::centred(
            self.get_json(&q.build())?,
            self.id(),
            center_item_id,
            rating_key,
        ))
    }

    /// POST /playQueues for a PLAYLIST — `playlistID=<id>` with the playlist's own `uri`, so the
    /// queue is the playlist's items in the playlist's order, starting at `rating_key` (its first
    /// item, or whichever one was chosen). A separate function rather than a flag on
    /// [`Self::create_play_queue`]: a playlist queue has no `continuous`, and the item queue's
    /// options (shuffle, the extras rule) are its own.
    pub fn create_playlist_queue(
        &self,
        machine_id: &str,
        playlist_id: &str,
        rating_key: &str,
        session: &str,
    ) -> Option<PlayQueueResult> {
        let q = self.playback_identity(playlist_queue_query(machine_id, playlist_id, rating_key, session));
        Some(PlayQueueResult::of(
            self.post_json(&q.build())?,
            self.id(),
            rating_key,
        ))
    }
}

/// The playlist queue's request, before the client's identity is added — split out so the
/// parameters are graded on the host.
fn playlist_queue_query(machine_id: &str, playlist_id: &str, rating_key: &str, session: &str) -> QueryBuilder {
    let uri = format!("server://{machine_id}/com.plexapp.plugins.library/playlists/{playlist_id}/items");
    QueryBuilder::new("/playQueues")
        .str("type", "video")
        .str("playlistID", playlist_id)
        .str("uri", &uri)
        .str("key", &format!("/library/metadata/{rating_key}"))
        .int("shuffle", 0)
        .int("repeat", 0)
        .str("X-Plex-Session-Identifier", session)
}

/// How many rows either side of the centre a continuation asks for (`window`). A projected row is
/// ~300 bytes, so 25 + 1 + 25 is ~15 KB — and it is the rows the queue panel lists.
pub const QUEUE_WINDOW: i64 = 25;

/// The POST /playQueues query without its identity headers — split out so a host test grades the
/// exact parameters (`continuous` dropped when false, `shuffle` the flag) without a server.
fn play_queue_query(uri: &str, continuous: bool, shuffle: bool) -> QueryBuilder {
    QueryBuilder::new("/playQueues")
        .str("type", "video")
        .str("uri", uri)
        .opt_int("continuous", i64::from(continuous))
        .int("shuffle", i64::from(shuffle))
        .int("repeat", 0)
}

/// The GET /playQueues/{id} query: a window of [`QUEUE_WINDOW`] rows each side of the centre.
fn fetch_queue_query(play_queue_id: i64, center_item_id: i64) -> QueryBuilder {
    QueryBuilder::new(&format!("/playQueues/{play_queue_id}"))
        .opt_int("center", center_item_id)
        .int("window", QUEUE_WINDOW)
        .int("includeBefore", 1)
        .int("includeAfter", 1)
}

/// One retained row of the play queue: everything a queue list draws, plus everything
/// `crate::route::request_play` needs to START that row — and nothing else.
///
/// The projection is the whole point. A `Metadata` row carries the entire Media/Part/Stream/Role
/// tree, this runs on the resolve worker of a 32-bit TV, and a `continuous=1` queue can be a whole
/// show — so the rows are retained ONLY in this shape. Field names mirror `route::UpNext` (which
/// is built from one of these) rather than the wire: `dur_ms` is `duration`, `resume_ms` is
/// `viewOffset`, `part`/`vcodec`/`acodec` come from `Media[0]`/`Media[0].Part[0]` exactly as the
/// single-successor projection always did.
///
/// NOT episode-gated: a queue row may be a movie, and the list must be able to show it. The
/// "episodes only" rule belongs to the one-item Up Next control, not to the queue.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct QueueRow {
    /// `playQueueItemID` — identity WITHIN the queue. A ratingKey can repeat in a queue (the same
    /// item queued twice), a playQueueItemID cannot, so every lookup keys off this. 0 = a server
    /// that omitted the field.
    pub item_id: i64,
    /// The server that ISSUED this queue, and so the server `rk` is a key on. A PlayQueue is
    /// per-server by construction (it is created with a `server://{machineIdentifier}/…` uri), so
    /// every row of one shares this — but the ratingKey FALLBACK below compares against a key
    /// supplied by the caller, and that one can come from anywhere.
    pub sid: super::ServerId,
    pub rk: String,
    /// `type` — movie | episode | clip …
    pub kind: String,
    /// the item's own title (the episode title for an episode)
    pub title: String,
    /// `grandparentTitle` — the show, empty for a movie
    pub show_title: String,
    /// `parentIndex` — season number (0 for a movie)
    pub season: i64,
    pub index: i64,
    pub thumb: String,
    /// `grandparentThumb` — the SHOW's portrait poster (empty for a movie). Retained beside
    /// `thumb` because the two are different SHAPES: an episode's `thumb` is a landscape still.
    /// Nothing draws it today — the Up Next tile shows the STILL, since the successor is another
    /// episode of the show the viewer is already watching — so this is the queue row's honest
    /// parse of the container and not a promise about a screen.
    pub poster: String,
    pub dur_ms: i64,
    /// `viewOffset` — the resume point, 0 = unwatched/from the start
    pub resume_ms: i64,
    /// `viewCount > 0` — watched at least once (the queue panel's watched mark)
    pub watched: bool,
    /// `Media[0].Part[0].key` — the direct-play part
    pub part: String,
    pub vcodec: String,
    pub acodec: String,
}

impl QueueRow {
    /// Consume a queue `Metadata` row into its lean projection. BY VALUE: the strings are moved,
    /// not cloned, and the rest of the row's tree is dropped as this returns.
    fn of(m: super::models::Metadata, sid: super::ServerId) -> QueueRow {
        // `Media[0]` / `Media[0].Part[0]`, the same pick `Metadata::first_part` makes — by value,
        // so the other versions and their Stream lists die here.
        let (vcodec, acodec, part) = match m.media.into_iter().next() {
            Some(md) => (
                md.video_codec,
                md.audio_codec,
                md.part
                    .into_iter()
                    .next()
                    .map(|p| p.key)
                    .unwrap_or_default(),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        QueueRow {
            item_id: m.play_queue_item_id,
            sid,
            rk: m.rating_key,
            kind: m.kind,
            title: m.title,
            show_title: m.grandparent_title,
            season: m.parent_index,
            index: m.index,
            thumb: m.thumb,
            poster: m.grandparent_thumb,
            dur_ms: m.duration,
            resume_ms: m.view_offset,
            watched: m.view_count > 0,
            part,
            vcodec,
            acodec,
        }
    }
}

/// Where a row sits in the queue — THE identity rule, in one place, because everything that
/// points at a queue row needs the same answer (the successor lookup below, and whatever draws
/// "you are here" in a queue list).
///
/// Identity is `playQueueItemID`, not `ratingKey`: a queue may legitimately hold the same item
/// twice, and matching on the rating key would then pick the wrong row. The rating key is only the
/// fallback for a server that omitted the per-row id — for a queue built FROM this item the two
/// agree, and losing the row entirely is the worse failure.
pub fn queue_index_of(
    items: &[QueueRow],
    item_id: i64,
    sid: ServerId,
    rating_key: &str,
) -> Option<usize> {
    let by_id = (item_id != 0)
        .then(|| items.iter().position(|r| r.item_id == item_id))
        .flatten();
    by_id.or_else(|| {
        items
            .iter()
            .position(|r| super::same_item((r.sid, &r.rk), (sid, rating_key)))
    })
}

/// The queue row that follows the selected one — None at the end of the queue, and None when the
/// selection matches no row at all (handing back `items[1]` there would start something the user
/// was never watching).
pub fn next_after<'a>(
    items: &'a [QueueRow],
    selected_item_id: i64,
    sid: ServerId,
    rating_key: &str,
) -> Option<&'a QueueRow> {
    items.get(queue_index_of(items, selected_item_id, sid, rating_key)? + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::models::Envelope;

    /// The server the fixture queues belong to. A PlayQueue is per-server by construction, so one
    /// slot for a whole queue is exactly the real shape.
    const Q: ServerId = ServerId::from_raw(0);

    /// one queue row: (playQueueItemID, ratingKey) on server [`Q`]
    fn row(item_id: i64, rk: &str) -> QueueRow {
        QueueRow {
            item_id,
            sid: Q,
            rk: rk.to_string(),
            ..Default::default()
        }
    }
    fn rks(items: &[QueueRow]) -> Vec<&str> {
        items.iter().map(|r| r.rk.as_str()).collect()
    }

    #[test]
    fn a_playlist_queue_names_the_playlist_and_starts_at_the_item() {
        let path = playlist_queue_query("m1", "77", "1001", "sess").build();
        assert!(path.starts_with("/playQueues?"));
        for part in ["type=video", "playlistID=77", "shuffle=0", "repeat=0"] {
            assert!(path.contains(part), "{part} in {path}");
        }
        assert!(path.contains("playlists%2F77%2Fitems") || path.contains("playlists/77/items"), "{path}");
        assert!(path.contains("metadata%2F1001") || path.contains("metadata/1001"), "{path}");
        assert!(!path.contains("continuous"), "a playlist queue is the playlist, nothing after it");
    }

    #[test]
    fn trailer_play_queues_omit_continuous() {
        use super::super::client::QueryBuilder;
        let with = QueryBuilder::new("/playQueues")
            .opt_int("continuous", 1)
            .build();
        let without = QueryBuilder::new("/playQueues")
            .opt_int("continuous", 0)
            .build();
        assert!(with.contains("continuous=1"));
        assert!(
            !without.contains("continuous"),
            "trailer sessions must not send continuous=1"
        );
    }
    /// **The shuffle POST and the continuation GET, as they go on the wire.** A shuffle names the
    /// container with `shuffle=1` and no `continuous`; the ordinary queue is unchanged (`shuffle=0`,
    /// `continuous=1`); the GET asks for a window either side of the row it continues on.
    #[test]
    fn shuffle_and_continuation_queries() {
        let plain = play_queue_query("server://m/com.plexapp.plugins.library/library/metadata/7", true, false).build();
        assert!(plain.contains("continuous=1") && plain.contains("shuffle=0"), "{plain}");
        let shuffled = play_queue_query("server://m/com.plexapp.plugins.library/library/metadata/70", false, true).build();
        assert!(shuffled.contains("shuffle=1"), "{shuffled}");
        assert!(!shuffled.contains("continuous"), "a shuffled queue is not continuous: {shuffled}");
        assert!(shuffled.contains("metadata%2F70"), "the container is the uri: {shuffled}");
        let get = fetch_queue_query(41, 9).build();
        assert!(get.starts_with("/playQueues/41?"), "{get}");
        assert!(get.contains("center=9") && get.contains(&format!("window={QUEUE_WINDOW}")), "{get}");
        assert!(!fetch_queue_query(41, 0).build().contains("center"), "no centre: the server's own");
    }

    /// **A continuation reads the window with OUR selection.** The server still has the previous
    /// row selected (the timeline report that moves it has not been sent), so `next` must follow
    /// the centre we asked for, not `playQueueSelectedItemID` — and a centre the window does not
    /// hold has no successor rather than the window's second row.
    #[test]
    fn a_centred_window_follows_the_requested_row() {
        let body = r#"{"MediaContainer":{"playQueueID":41,"playQueueSelectedItemID":1,
            "playQueueSelectedItemOffset":0,"playQueueTotalCount":4,"Metadata":[
            {"playQueueItemID":1,"ratingKey":"a","type":"episode","viewCount":1},
            {"playQueueItemID":2,"ratingKey":"b","type":"episode"},
            {"playQueueItemID":3,"ratingKey":"c","type":"episode"},
            {"playQueueItemID":4,"ratingKey":"d","type":"episode"}]}}"#;
        let mc = || serde_json::from_str::<Envelope>(body).unwrap().media_container;
        let r = PlayQueueResult::centred(mc(), Q, 2, "b");
        assert_eq!((r.id, r.selected_item_id, r.remaining), (41, 2, 2));
        assert_eq!(r.next.as_ref().map(|n| n.rk.as_str()), Some("c"), "after OUR row, not the server's");
        assert_eq!(rks(&r.items), ["a", "b", "c", "d"]);
        assert!(r.items[0].watched && !r.items[1].watched, "viewCount is the watched mark");
        let last = PlayQueueResult::centred(mc(), Q, 4, "d");
        assert!(last.next.is_none() && last.remaining == 0);
        let gone = PlayQueueResult::centred(mc(), Q, 99, "zz");
        assert!(gone.next.is_none(), "an unknown centre has no successor");
    }

    /// A response body through the SHIPPED mapping — the same call `create_play_queue` makes once
    /// its POST returns, so these tests cannot pass on a projection the app does not use.
    fn result(body: &str, rating_key: &str) -> PlayQueueResult {
        let mc = serde_json::from_str::<Envelope>(body)
            .expect("a PMS body parses")
            .media_container;
        PlayQueueResult::of(mc, Q, rating_key)
    }

    /// The whole up-next extraction. Every case here is one the live server actually produces:
    /// a mid-season episode (a successor exists), a season finale with nothing after it
    /// (`playQueueTotalCount: 1` — verified on a show's last episode), and a movie, whose
    /// `continuous=1` queue is just itself.
    #[test]
    fn the_successor_is_the_row_after_the_selected_one() {
        let q = vec![row(13083, "1804"), row(13084, "1805"), row(13085, "1806")];
        let next = next_after(&q, 13083, Q, "1804").expect("a mid-queue item has a successor");
        assert_eq!(next.rk, "1805");

        // a queue of one — the season finale / movie case
        let solo = vec![row(13092, "774")];
        assert!(next_after(&solo, 13092, Q, "774").is_none());
        // ...and the LAST row of a longer queue is the same thing
        let q = vec![row(1, "a"), row(2, "b")];
        assert!(next_after(&q, 2, Q, "b").is_none());
    }

    #[test]
    fn the_selected_row_is_found_by_queue_item_id_not_rating_key() {
        // A queue holding the same episode twice: matching on the ratingKey would pick the FIRST
        // occurrence and hand back its successor, which is the wrong episode.
        let q = vec![row(1, "dup"), row(2, "mid"), row(3, "dup"), row(4, "after")];
        let next = next_after(&q, 3, Q, "dup").expect("the second occurrence has a successor");
        assert_eq!(next.rk, "after");
    }

    #[test]
    fn a_missing_queue_item_id_falls_back_to_the_rating_key() {
        // Rows with no playQueueItemID (0) — the successor must still be found, or the feature
        // silently disappears on a server that trims the field.
        let q = vec![row(0, "1804"), row(0, "1805")];
        let next = next_after(&q, 0, Q, "1804").expect("the rating key is the fallback identity");
        assert_eq!(next.rk, "1805");
    }

    /// The FALLBACK is the one identity here that a second server can spoof. `playQueueItemID` is
    /// per-queue and safe; the rating key is not, so it is compared as `(server, key)` — otherwise
    /// a queue built on the share and a playing rk read off our own server (or the reverse) would
    /// "find" a row and auto-advance to something the viewer never queued.
    #[test]
    fn the_rating_key_fallback_is_scoped_to_the_queues_own_server() {
        let other = ServerId::from_raw(1);
        let q = vec![row(0, "1804"), row(0, "1805")];
        assert!(
            next_after(&q, 0, other, "1804").is_none(),
            "another server's 1804 is not this row"
        );
        assert_eq!(
            queue_index_of(&q, 0, Q, "1804"),
            Some(0),
            "…and the queue's OWN 1804 still is"
        );

        // the primary key is unaffected: a playQueueItemID is minted by the queue itself, so it
        // needs no scoping and must keep working even when the caller names another server
        let byid = vec![row(13083, "1804"), row(13084, "1805")];
        assert_eq!(queue_index_of(&byid, 13084, other, "zzz"), Some(1));
    }

    #[test]
    fn an_unrecognised_selection_yields_nothing_rather_than_the_first_row() {
        // Neither identity matches: returning `items[0]`'s successor here would start an episode
        // the user was never watching.
        let q = vec![row(1, "a"), row(2, "b")];
        assert!(next_after(&q, 99, Q, "zzz").is_none());
        assert!(
            next_after(&[], 1, Q, "a").is_none(),
            "an empty queue is not a panic"
        );
    }

    /// A real `continuous=1` response — three episodes, the first selected — through the shipped
    /// mapping. Shapes that are here on purpose: PMS string-encodes some numbers (`duration`,
    /// `viewOffset`) and not others, an episode can carry MULTIPLE `Media[]` versions (4K + 1080p
    /// — the projection takes `[0]`, the same pick the single-successor code always made), and the
    /// rows carry the Stream/Role tree that the lean row has nowhere to put.
    #[test]
    fn a_real_playqueue_body_keeps_every_row_as_a_lean_projection() {
        let q = result(
            r#"{"MediaContainer":{"size":3,"playQueueID":40213,"playQueueSelectedItemID":13083,
              "playQueueSelectedItemOffset":0,"playQueueTotalCount":3,"Metadata":[
              {"playQueueItemID":13083,"ratingKey":"1804","type":"episode","title":"Pilot",
               "grandparentTitle":"Example Show","parentIndex":1,"index":1,
               "thumb":"/library/metadata/1804/thumb/1781586780","duration":"3273248",
               "viewOffset":"142000","Role":[{"tag":"Jennifer Aniston"}],
               "Media":[{"videoCodec":"hevc","audioCodec":"eac3",
                 "Part":[{"id":3130,"key":"/library/parts/3130/1781467224/file.mkv",
                   "Stream":[{"id":1,"streamType":1},{"id":2,"streamType":2}]}]},
                {"videoCodec":"h264","audioCodec":"aac",
                 "Part":[{"id":3134,"key":"/library/parts/3134/1781468203/file.mkv"}]}]},
              {"playQueueItemID":13084,"ratingKey":"1805","type":"episode","title":"A Seat at the Table",
               "grandparentTitle":"Example Show","parentIndex":1,"index":2,"duration":3120000,
               "Media":[{"videoCodec":"h264","audioCodec":"ac3",
                 "Part":[{"key":"/library/parts/3140/1781467999/file.mkv"}]}]},
              {"playQueueItemID":13085,"ratingKey":"1806","type":"episode","title":"Chaos Is the New Cocaine",
               "grandparentTitle":"Example Show","parentIndex":1,"index":3,
               "Media":[{"videoCodec":"h264","audioCodec":"ac3","Part":[{"key":"/p/3.mkv"}]}]}]}}"#,
            "1804",
        );
        assert_eq!(
            rks(&q.items),
            ["1804", "1805", "1806"],
            "the WHOLE window is retained, not just the successor"
        );
        assert_eq!(
            (q.id, q.selected_item_id, q.remaining),
            (40213, 13083, 2),
            "the ids and the count still land"
        );

        let first = &q.items[0];
        assert_eq!(first.item_id, 13083);
        assert_eq!(first.kind, "episode");
        assert_eq!(first.title, "Pilot");
        assert_eq!(first.show_title, "Example Show");
        assert_eq!((first.season, first.index), (1, 1));
        assert_eq!(first.thumb, "/library/metadata/1804/thumb/1781586780");
        assert_eq!(
            (first.dur_ms, first.resume_ms),
            (3273248, 142000),
            "string-encoded numbers still land"
        );
        assert_eq!(
            first.part, "/library/parts/3130/1781467224/file.mkv",
            "Media[0].Part[0].key"
        );
        assert_eq!(
            (first.vcodec.as_str(), first.acodec.as_str()),
            ("hevc", "eac3"),
            "Media[0], not the 1080p version"
        );

        // the successor is exactly what it was when it was the only thing kept — and it is the
        // retained row after the selected one, not a separately-derived answer
        let next = q.next.as_ref().expect("the selected row has a successor");
        assert_eq!((next.rk.as_str(), next.index), ("1805", 2));
        assert_eq!(
            next.part, "/library/parts/3140/1781467999/file.mkv",
            "the successor is start-able"
        );
        assert_eq!(next.dur_ms, 3120000, "…and a plain JSON number lands too");
        let at = queue_index_of(&q.items, q.selected_item_id, Q, "1804")
            .expect("the playing row is locatable");
        assert_eq!(
            (at, q.items[at + 1].rk.as_str()),
            (0, next.rk.as_str()),
            "next IS items[selected+1]"
        );
    }

    /// Two rows the LIST must survive that the one-item Up Next control would have refused: a
    /// movie (a `continuous=1` movie queue is just itself — the list still has to be able to draw
    /// it) and a row with no `Media` at all, which must project to empty strings, not a panic.
    #[test]
    fn a_movie_row_and_a_media_less_row_both_project() {
        let q = result(
            r#"{"MediaContainer":{"playQueueSelectedItemID":900,"Metadata":[
              {"playQueueItemID":900,"ratingKey":"774","type":"movie","title":"Sinners",
               "Media":[{"videoCodec":"hevc","audioCodec":"truehd","Part":[{"key":"/p/774.mkv"}]}]},
              {"playQueueItemID":901,"ratingKey":"775","type":"movie","title":"No Media Here"}]}}"#,
            "774",
        );
        let items = &q.items;
        assert_eq!(
            items[0].kind, "movie",
            "the projection is NOT episode-gated"
        );
        assert_eq!(items[0].show_title, "", "a movie has no grandparentTitle");
        assert_eq!(items[0].part, "/p/774.mkv");
        let bare = &items[1];
        assert_eq!(
            (
                bare.part.as_str(),
                bare.vcodec.as_str(),
                bare.acodec.as_str()
            ),
            ("", "", "")
        );
        assert_eq!(bare.rk, "775", "the rest of the row still projects");
    }
}

/// What a POST /playQueues gives the app: the two ids the `/:/timeline` report carries, plus the
/// queue's own view of what comes next. Kept as a struct rather than a tuple because `next` made
/// the third and fourth members meaningless positionally.
pub struct PlayQueueResult {
    pub id: i64,
    pub selected_item_id: i64,
    /// items queued AFTER the one now playing (0 = this is the last)
    pub remaining: i64,
    /// The returned window of the queue, projected — the row now playing INCLUDED, in queue order,
    /// so a list can show where playback sits. Its length may DIFFER from `remaining + 1` in both
    /// directions: the window can be a slice of a long queue, and it also carries the rows BEFORE
    /// the selected one when `playQueueSelectedItemOffset > 0`. `playQueueTotalCount` is the whole
    /// queue; this is what the server actually sent.
    pub items: Vec<QueueRow>,
    /// the item that plays next, or None at the end of the queue (and for a movie, whose
    /// continuous queue is just itself — verified live: total count 1). A copy of the `items` row
    /// after the selected one; one lean row is worth not making every caller redo the lookup.
    pub next: Option<QueueRow>,
}

impl PlayQueueResult {
    /// The WHOLE response→result mapping, kept out of `create_play_queue` so the host tests grade
    /// the code that ships instead of a copy of it (only the request build is left up there).
    fn of(
        mc: super::models::MediaContainer,
        sid: super::ServerId,
        rating_key: &str,
    ) -> PlayQueueResult {
        // Remaining AFTER the item being played, from the whole-queue counters (the returned
        // `Metadata[]` can be a window). Clamped: a server that omits either counter must read as
        // "nothing after this", not as a negative count the UI would then format.
        let remaining = (mc.play_queue_total_count - mc.play_queue_selected_item_offset - 1).max(0);
        let selected = mc.play_queue_selected_item_id;
        // Project BY VALUE: every `Metadata` row is consumed into a `QueueRow` and its
        // Media/Part/Stream/Role tree dropped right here, on the worker. What the main thread then
        // holds for the length of the playback is a dozen scalars and strings per row.
        let items: Vec<QueueRow> = mc
            .metadata
            .into_iter()
            .map(|m| QueueRow::of(m, sid))
            .collect();
        let next = next_after(&items, selected, sid, rating_key).cloned();
        PlayQueueResult {
            id: mc.play_queue_id,
            selected_item_id: selected,
            remaining,
            items,
            next,
        }
    }

    /// A GET /playQueues/{id}?center=… answer, read with OUR selection: `center` is the row this
    /// playback is starting, which the server will only learn from the next timeline report, so
    /// the container's own `playQueueSelectedItemID` (the PREVIOUS item) must not decide `next`.
    /// `remaining` counts the window's rows after the centre — the whole-queue offset the server
    /// reports is the old selection's, not this one's.
    fn centred(
        mc: super::models::MediaContainer,
        sid: super::ServerId,
        center_item_id: i64,
        rating_key: &str,
    ) -> PlayQueueResult {
        let id = mc.play_queue_id;
        let items: Vec<QueueRow> = mc
            .metadata
            .into_iter()
            .map(|m| QueueRow::of(m, sid))
            .collect();
        let at = queue_index_of(&items, center_item_id, sid, rating_key);
        let remaining = at.map_or(0, |i| (items.len() - i - 1) as i64);
        let next = next_after(&items, center_item_id, sid, rating_key).cloned();
        PlayQueueResult {
            id,
            selected_item_id: center_item_id,
            remaining,
            items,
            next,
        }
    }
}
