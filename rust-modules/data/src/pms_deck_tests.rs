//! Continue Watching keeps only what can be continued (`spent_deck_entry`), and the in-flight read
//! the deck's refetch schedule asks (`in_flight`).

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

fn deck(body: &str) -> plx_plex::plex::MediaContainer {
    let wrapped = format!(r#"{{"MediaContainer":{{"Hub":[{{"type":"mixed","hubIdentifier":"home.continue","Metadata":[{body}]}}]}}}}"#);
    serde_json::from_str::<plx_plex::plex::Envelope>(&wrapped).expect("the deck parses").media_container
}

fn kept(body: &str) -> Vec<String> {
    let build = project(&plx_plex::plex::MediaContainer::default(), &deck(body), sid(0));
    build.cw.iter().map(|c| c.m.rk.clone()).collect()
}

const POSTER: &str = r#""thumb":"/t.jpg","art":"/a.jpg""#;

#[test]
fn a_finished_movie_the_server_still_lists_leaves_the_deck() {
    let body = format!(
        r#"{{"ratingKey":"1","type":"movie","title":"done","viewCount":1,"duration":6000,{POSTER}}},
           {{"ratingKey":"2","type":"movie","title":"to the end","viewCount":2,"viewOffset":6000,"duration":6000,{POSTER}}},
           {{"ratingKey":"3","type":"movie","title":"half","viewOffset":3000,"duration":6000,{POSTER}}},
           {{"ratingKey":"4","type":"movie","title":"rewatching","viewCount":1,"viewOffset":3000,"duration":6000,{POSTER}}}"#
    );
    assert_eq!(kept(&body), ["3", "4"], "only what has a resume point left stays");
}

#[test]
fn a_next_episode_stays_even_on_a_rewatch_but_an_unavailable_one_goes() {
    let body = format!(
        r#"{{"ratingKey":"10","type":"episode","title":"next","{POSTER_KEY}":"/p.jpg",{POSTER}}},
           {{"ratingKey":"11","type":"episode","title":"rewatch next","viewCount":1,{POSTER}}},
           {{"ratingKey":"12","type":"episode","title":"missing file","Media":[{{"Part":[{{"id":5}}]}}],{POSTER}}},
           {{"ratingKey":"13","type":"episode","title":"no parts","Media":[{{"Part":[]}}],{POSTER}}},
           {{"ratingKey":"14","type":"episode","title":"playable","Media":[{{"Part":[{{"key":"/library/parts/1/1/f.mkv"}}]}}],{POSTER}}}"#,
        POSTER_KEY = "grandparentThumb"
    );
    assert_eq!(kept(&body), ["10", "11", "14"]);
}

#[test]
fn a_show_with_every_episode_watched_has_no_next_episode() {
    let body = format!(
        r#"{{"ratingKey":"20","type":"show","title":"all seen","leafCount":8,"viewedLeafCount":8,{POSTER}}},
           {{"ratingKey":"21","type":"show","title":"part seen","leafCount":8,"viewedLeafCount":3,{POSTER}}},
           {{"ratingKey":"22","type":"season","title":"season seen","leafCount":4,"viewedLeafCount":4,{POSTER}}}"#
    );
    assert_eq!(kept(&body), ["21"]);
}

#[test]
fn in_flight_is_the_single_flight_latch_of_any_source() {
    let _g = plx_base::testlock::serial();
    let (mut state, adapter) = (PmsState::default(), Arc::new(PmsAdapter::default()));
    seed_for_test(&mut state, &adapter, 3, HubState::Ready);
    assert!(!in_flight(&state), "a settled source asks nothing");
    state.srcs[0].fetching = true;
    assert!(in_flight(&state), "its fetch is out");
}
