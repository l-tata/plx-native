//! Home's own shelves ([`HomeExtras`] and each source's genre rows): where the merge places them,
//! what it keeps off them, and that they never disturb the server shelves' accounting.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

fn channel(rk: &str) -> Arc<PmsMovie> {
    Arc::new(PmsMovie { sid: ServerId::UNSET, kind: KIND_CHANNEL, rk: rk.into(), title: rk.into(), ..Default::default() })
}

fn ids(hubs: &[HubRow]) -> Vec<&str> {
    hubs.iter().map(|h| h.hub_id.as_str()).collect()
}

fn keys(items: &[Arc<PmsMovie>], hub: &HubRow) -> Vec<String> {
    items[hub.start..hub.start + hub.len].iter().map(|m| m.rk.clone()).collect()
}

fn with_genres(mut b: SourceBuild, slot: u16, rks: &[&str]) -> SourceBuild {
    b.genres = Some(rks.iter().map(|r| row(slot, r)).collect());
    b
}

#[test]
fn the_app_shelves_follow_the_deck_in_a_fixed_order() {
    let srcs = [src(0, "", HubState::Ready, Some(with_genres(
        built(0, &[(9, "cw")], vec![shelf(0, "Recent", "home.movies.recent", &["r1"])]), 0, &["g1"])))];
    let extras = HomeExtras { on_now: vec![channel("12")], watchlist: vec![row(0, "w1")] };
    let (_, hubs, _) = merge_with_scope(&srcs, &BrowseScope::standalone(), &extras);
    assert_eq!(ids(&hubs), ["home.continue", WATCHLIST_HUB, ON_NOW_HUB, GENRES_HUB, "home.movies.recent"]);
    assert!(hubs.iter().all(|h| h.source.is_empty()), "an app shelf credits nobody");
}

#[test]
fn an_empty_app_shelf_draws_no_heading() {
    let srcs = [src(0, "", HubState::Ready, Some(built(0, &[], vec![shelf(0, "Recent", "home.movies.recent", &["r1"])])))];
    let (_, hubs, _) = merge_with_scope(&srcs, &BrowseScope::standalone(), &HomeExtras::default());
    assert_eq!(ids(&hubs), ["home.movies.recent"]);
}

#[test]
fn the_genre_shelf_shows_only_what_no_other_shelf_does() {
    let srcs = [src(0, "", HubState::Ready, Some(with_genres(
        built(0, &[(9, "cw")], vec![shelf(0, "Recent", "home.movies.recent", &["r1"])]),
        0, &["cw", "r1", "w1", "fresh"])))];
    let extras = HomeExtras { on_now: Vec::new(), watchlist: vec![row(0, "w1")] };
    let (items, hubs, _) = merge_with_scope(&srcs, &BrowseScope::standalone(), &extras);
    let genres = hubs.iter().find(|h| h.hub_id == GENRES_HUB).expect("the genre shelf");
    assert_eq!(keys(&items, genres), ["fresh"], "the deck, the watchlist and the server shelves already show the rest");
}

#[test]
fn the_genre_shelf_takes_turns_between_sources() {
    let srcs = [
        src(0, "", HubState::Ready, Some(with_genres(built(0, &[], Vec::new()), 0, &["a1", "a2", "a3"]))),
        src(1, "friend", HubState::Ready, Some(with_genres(built(1, &[], Vec::new()), 1, &["b1"]))),
    ];
    let (items, hubs, _) = merge_with_scope(&srcs, &BrowseScope::standalone(), &HomeExtras::default());
    assert_eq!(keys(&items, &hubs[0]), ["a1", "b1", "a2", "a3"]);
}

#[test]
fn an_unpinned_librarys_titles_stay_off_the_app_shelves_but_channels_do_not() {
    let srcs: [Src; 0] = [];
    let mut w = (*row(0, "w1")).clone();
    w.sec = 3;
    let extras = HomeExtras { on_now: vec![channel("5")], watchlist: vec![Arc::new(w)] };
    let scope = BrowseScope { sections_gen: 0, pins: vec![(sid(0), 3, false)] };
    let (_, hubs, _) = merge_with_scope(&srcs, &scope, &extras);
    assert_eq!(ids(&hubs), [ON_NOW_HUB], "a channel belongs to no library");
}

#[test]
fn an_app_shelf_keeps_one_identity_whichever_server_leads_it() {
    let srcs: [Src; 0] = [];
    for lead in [0, 1] {
        let extras = HomeExtras { on_now: Vec::new(), watchlist: vec![row(lead, "w"), row(1 - lead, "v")] };
        let (items, hubs, _) = merge_with_scope(&srcs, &BrowseScope::standalone(), &extras);
        assert_eq!(
            stable_hub_identity(&hubs[0], &items),
            Some(HubIdentity::Identifier { sid: ServerId::UNSET, id: WATCHLIST_HUB, key: "" }),
        );
    }
}

#[test]
fn the_app_shelves_are_never_counted_as_left_off_by_the_card_bound() {
    let srcs = [src(0, "", HubState::Ready, Some(built(0, &[(1, "cw")], vec![shelf(0, "Recent", "home.movies.recent", &["r1"])])))];
    let extras = HomeExtras { on_now: vec![channel("1"), channel("2")], watchlist: vec![row(0, "w1")] };
    let build = merge_with_scope(&srcs, &BrowseScope::standalone(), &extras);
    assert_eq!(bound_overflow(&srcs, &BrowseScope::standalone(), &build), (0, 0));
}

#[test]
fn an_unchanged_on_now_shelf_does_not_republish_home() {
    let _g = plx_base::testlock::serial();
    let mut owner = Owner::default();
    seed(&mut owner.state, vec![src(0, "", HubState::Ready, Some(built(0, &[], Vec::new())))]);
    let card = |title: &str| PmsMovie { kind: KIND_CHANNEL, rk: "4".into(), title: title.into(), ..Default::default() };
    let scope = BrowseScope::standalone();
    assert!(set_on_now(&mut owner.state, vec![card("news")], &scope), "a new shelf publishes");
    let gen = owner.state.catalog_gen;
    assert!(!set_on_now(&mut owner.state, vec![card("news")], &scope), "the same cards again: nothing to publish");
    assert_eq!(owner.state.catalog_gen, gen);
    assert!(set_on_now(&mut owner.state, vec![card("film")], &scope), "the programme changed");
    assert_eq!(hub_title(&owner.state, 0), plx_platform::i18n::msg::browse_home_on_now());
    assert!(set_on_now(&mut owner.state, Vec::new(), &scope));
    assert_eq!(hub_count(&owner.state), 0, "Live TV turned off: the shelf goes");
}

fn copy(rk: &str, guid: &str) -> PmsMovie {
    PmsMovie { guid: guid.into(), ..(*row(0, rk)).clone() }
}

#[test]
fn a_watchlist_landing_publishes_the_shelf_and_the_membership() {
    let _g = plx_base::testlock::serial();
    let mut owner = Owner::default();
    seed(&mut owner.state, vec![src(0, "", HubState::Ready, Some(built(0, &[], Vec::new())))]);
    assert_eq!(hubs_snapshot(&owner.state).view().on_watchlist("plex://movie/a"), None, "unknown until read");
    *owner.adapter.watchlist.lock().unwrap() = Some(crate::watchlist::Landing {
        gen: owner.state.watchlist.gen,
        build: Some(crate::watchlist::Build {
            members: crate::watchlist::Membership::new(["plex://movie/a".to_owned(), "plex://movie/z".to_owned()]),
            rows: vec![copy("1", "plex://movie/a")],
        }),
    });
    pump(&mut owner.state, &owner.adapter, 0.0);
    assert_eq!(hub_title(&owner.state, 0), plx_platform::i18n::msg::browse_home_watchlist());
    let snap = hubs_snapshot(&owner.state);
    assert_eq!(snap.view().on_watchlist("plex://movie/z"), Some(true), "a title no library holds is still on the list");
    assert_eq!(snap.view().on_watchlist("plex://movie/q"), Some(false));
}

#[test]
fn a_landing_from_before_a_profile_switch_is_dropped() {
    let _g = plx_base::testlock::serial();
    let mut owner = Owner::default();
    let stale = owner.state.watchlist.gen;
    reset(&mut owner.state, &owner.adapter);
    *owner.adapter.watchlist.lock().unwrap() = Some(crate::watchlist::Landing {
        gen: stale,
        build: Some(crate::watchlist::Build { members: Default::default(), rows: vec![copy("1", "plex://movie/a")] }),
    });
    assert!(!land_watchlist(&mut owner.state, &owner.adapter));
    assert!(owner.state.extras.watchlist.is_empty());
}

#[test]
fn an_edit_moves_the_shelf_and_the_membership_at_once() {
    let _g = plx_base::testlock::serial();
    let mut owner = Owner::default();
    seed(&mut owner.state, vec![src(0, "", HubState::Ready, Some(built(0, &[], Vec::new())))]);
    owner.state.watchlist.members = Some(Arc::new(crate::watchlist::Membership::new(["plex://movie/a".to_owned()])));
    owner.state.extras.watchlist = vec![Arc::new(copy("1", "plex://movie/a"))];
    let scope = BrowseScope::standalone();
    assert!(edit_watchlist(&mut owner.state, &owner.adapter, "plex://movie/b", true, vec![copy("2", "")], &scope));
    assert_eq!(rks(&owner.state, 0), ["2", "1"], "the added title leads the shelf");
    assert_eq!(hubs_snapshot(&owner.state).view().on_watchlist("plex://movie/b"), Some(true));
    assert!(edit_watchlist(&mut owner.state, &owner.adapter, "plex://movie/a", false, Vec::new(), &scope));
    assert_eq!(rks(&owner.state, 0), ["2"]);
    assert_eq!(hubs_snapshot(&owner.state).view().on_watchlist("plex://movie/a"), Some(false));
    assert!(!edit_watchlist(&mut owner.state, &owner.adapter, "local://9", true, Vec::new(), &scope), "not a catalog title");
}

#[test]
fn a_fetch_that_did_not_build_the_genre_shelf_keeps_the_last_one() {
    let mut s = src(0, "", HubState::Ready, None);
    landed_ok(&mut s, with_genres(built(0, &[], Vec::new()), 0, &["g1"]));
    let built_at = s.genres_at_ms.expect("the shelf was built");
    landed_ok(&mut s, built(0, &[], Vec::new()));
    let kept: Vec<_> = s.last.as_ref().unwrap().genres.as_ref().unwrap().iter().map(|m| m.rk.clone()).collect();
    assert_eq!(kept, ["g1"], "a plain refresh keeps the genre rows");
    assert_eq!(s.genres_at_ms, Some(built_at), "and the shelf's own clock");
    landed_ok(&mut s, with_genres(built(0, &[], Vec::new()), 0, &[]));
    assert!(s.last.as_ref().unwrap().genres.as_ref().unwrap().is_empty(), "a rebuilt empty shelf replaces it");
}

#[test]
fn a_servers_video_playlists_become_playlist_cards() {
    let mc: plx_plex::plex::MediaContainer = serde_json::from_str(r#"{"Metadata":[
        {"type":"playlist","ratingKey":"77","title":"s77","composite":"/playlists/77/composite/1","leafCount":4,"playlistType":"video"},
        {"type":"playlist","ratingKey":"78","title":"empty","leafCount":0},
        {"type":"movie","ratingKey":"1","title":"not a playlist","leafCount":3}]}"#).unwrap();
    let rows = playlist_rows(&mc, sid(0));
    assert_eq!(rows.len(), 1, "an empty playlist and a non-playlist row are left off");
    assert_eq!((rows[0].kind, rows[0].rk.as_str(), rows[0].thumb.as_str(), rows[0].child_count),
        (KIND_PLAYLIST, "77", "/playlists/77/composite/1", 4));
    assert!(!item_has_menu_kind(KIND_PLAYLIST));
}

#[test]
fn the_playlists_shelf_follows_the_genre_shelf() {
    let mut b = with_genres(built(0, &[], vec![shelf(0, "Recent", "home.movies.recent", &["r1"])]), 0, &["g1"]);
    b.playlists = Some(vec![Arc::new(PmsMovie { kind: KIND_PLAYLIST, rk: "77".into(), title: "s77".into(), ..Default::default() })]);
    let srcs = [src(0, "", HubState::Ready, Some(b))];
    let (_, hubs, _) = merge_with_scope(&srcs, &BrowseScope::standalone(), &HomeExtras::default());
    assert_eq!(ids(&hubs), [GENRES_HUB, PLAYLISTS_HUB, "home.movies.recent"]);
}
