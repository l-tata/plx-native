use super::*;

const A: ServerId = ServerId::from_raw(0);
const B: ServerId = ServerId::from_raw(1);

fn airing(title: &str, episode: &str, sub: &str, cats: &[&str], year: Option<u16>) -> Airing {
    Airing {
        title: title.into(),
        episode: episode.into(),
        sub_title: sub.into(),
        categories: cats.iter().map(|c| c.to_string()).collect(),
        year,
        ..Default::default()
    }
}

fn film(sid: ServerId, rk: &str, title: &str, year: i64) -> Candidate {
    Candidate { sid: Some(sid), rk: rk.into(), kind: "movie".into(), title: title.into(), year, ..Default::default() }
}

fn ep(sid: ServerId, rk: &str, show: &str, s: i64, e: i64, title: &str) -> Candidate {
    Candidate { sid: Some(sid), rk: rk.into(), kind: "episode".into(), title: title.into(), show: show.into(), season: s, index: e, ..Default::default() }
}

fn hit(sid: ServerId, rk: &str) -> Option<Hit> {
    Some(Hit { sid, rk: rk.into() })
}

#[test]
fn an_airing_is_wanted_as_an_episode_when_it_has_episode_facts_and_as_a_film_otherwise() {
    assert_eq!(
        Want::of(&airing("Bluey", "S02E05", "Dance Mode", &[], None)),
        Some(Want::Episode { show: "Bluey".into(), season: Some(2), episode: Some(5), title: "Dance Mode".into() })
    );
    assert_eq!(
        Want::of(&airing("Bluey", "", "Dance Mode", &[], None)),
        Some(Want::Episode { show: "Bluey".into(), season: None, episode: None, title: "Dance Mode".into() })
    );
    assert_eq!(Want::of(&airing("Sintel", "", "", &[], Some(2010))), Some(Want::Film { title: "Sintel".into(), year: Some(2010) }));
    assert_eq!(
        Want::of(&airing("Sintel", "", "Director's cut", &["Movie"], None)),
        Some(Want::Film { title: "Sintel".into(), year: None }),
        "a guide that says Movie is believed over a sub-title"
    );
    assert_eq!(Want::of(&airing("  ", "", "", &[], None)), None, "nothing to look up");
    assert_eq!(Want::of(&airing("!!", "", "", &[], None)), None);
}

#[test]
fn episode_numbers_parse_in_the_spellings_guides_use() {
    assert_eq!(parse_episode("S02E05"), (Some(2), Some(5)));
    assert_eq!(parse_episode("s2e15"), (Some(2), Some(15)));
    assert_eq!(parse_episode("E07"), (None, Some(7)));
    assert_eq!(parse_episode("S03"), (Some(3), None));
    assert_eq!(parse_episode(""), (None, None));
}

#[test]
fn titles_normalise_punctuation_case_and_ampersands() {
    assert_eq!(normalise("Bob's Burgers"), normalise("Bobs Burgers"));
    assert_eq!(normalise("Star Trek: The Next Generation"), "star trek the next generation");
    assert_eq!(normalise("Tom & Jerry"), normalise("Tom and Jerry"));
    assert_eq!(normalise("  WALL·E "), "wall e");
    assert_ne!(normalise("The Office"), normalise("Office"), "articles are part of a title");
}

#[test]
fn a_film_matches_on_its_exact_title_and_its_year() {
    let rows = [film(A, "1", "Sintel", 2010), film(A, "2", "Sintel 2", 2012), film(A, "3", "Tears of Steel", 2012)];
    let want = |t: &str, y: Option<u16>| Want::Film { title: t.into(), year: y };
    assert_eq!(best(&want("Sintel", Some(2010)), &rows), hit(A, "1"));
    assert_eq!(best(&want("sintel", None), &rows), hit(A, "1"), "a unique title needs no year");
    assert_eq!(best(&want("Sintel", Some(2011)), &rows), hit(A, "1"), "a year either way");
    assert_eq!(best(&want("Sintel", Some(2015)), &rows), None, "a different year is a different film");
    assert_eq!(best(&want("Sinte", None), &rows), None, "never a prefix");
    assert_eq!(best(&want("Steel", None), &rows), None, "never a substring");
}

#[test]
fn two_films_of_one_title_need_the_year_to_tell_them_apart() {
    let rows = [film(A, "1", "Dune", 1984), film(A, "2", "Dune", 2021)];
    assert_eq!(best(&Want::Film { title: "Dune".into(), year: None }, &rows), None, "ambiguous: offer nothing");
    assert_eq!(best(&Want::Film { title: "Dune".into(), year: Some(2021) }, &rows), hit(A, "2"));
    // The same film on two servers is not ambiguous: the first server's copy is offered.
    let shared = [film(A, "7", "Dune", 2021), film(B, "9", "Dune", 2021)];
    assert_eq!(best(&Want::Film { title: "Dune".into(), year: None }, &shared), hit(A, "7"));
}

#[test]
fn a_film_never_matches_a_show_or_an_episode_of_that_name() {
    let rows = [Candidate { sid: Some(A), rk: "5".into(), kind: "show".into(), title: "Fargo".into(), ..Default::default() }];
    assert_eq!(best(&Want::Film { title: "Fargo".into(), year: None }, &rows), None);
}

#[test]
fn an_episode_matches_on_its_show_and_numbers() {
    let rows = [
        ep(A, "10", "Bluey", 2, 4, "Hammerbarn"),
        ep(A, "11", "Bluey", 2, 5, "Dance Mode"),
        ep(A, "12", "Bluey Jr", 2, 5, "Dance Mode"),
    ];
    let want = Want::Episode { show: "Bluey".into(), season: Some(2), episode: Some(5), title: String::new() };
    assert_eq!(best(&want, &rows), hit(A, "11"));
    let wrong = Want::Episode { show: "Bluey".into(), season: Some(3), episode: Some(5), title: "Dance Mode".into() };
    assert_eq!(best(&wrong, &rows), None, "numbers that disagree are not rescued by the title");
}

#[test]
fn an_episode_without_numbers_matches_on_its_own_title() {
    let rows = [ep(A, "11", "Bluey", 2, 5, "Dance Mode")];
    let want = Want::Episode { show: "bluey".into(), season: None, episode: None, title: "Dance mode".into() };
    assert_eq!(best(&want, &rows), hit(A, "11"));
    let untitled = Want::Episode { show: "Bluey".into(), season: None, episode: None, title: String::new() };
    assert_eq!(best(&untitled, &rows), None, "a show alone names no episode");
    let number_only = Want::Episode { show: "Bluey".into(), season: None, episode: Some(5), title: "Dance Mode".into() };
    assert_eq!(best(&number_only, &rows), hit(A, "11"));
}

#[test]
fn queries_ask_the_episode_title_before_the_show() {
    let want = Want::Episode { show: "Bluey".into(), season: Some(2), episode: Some(5), title: "Dance Mode".into() };
    assert_eq!(want.queries(), ["Dance Mode", "Bluey"]);
    assert_eq!(Want::Film { title: " Sintel ".into(), year: None }.queries(), ["Sintel"]);
    let rows = [Candidate { sid: Some(A), rk: "3".into(), kind: "show".into(), title: "Bluey".into(), ..Default::default() }];
    assert_eq!(show_of(&want, &rows).map(|c| c.rk.as_str()), Some("3"));
}

#[test]
fn keys_share_an_answer_between_reruns_and_tell_films_apart_by_year() {
    let a = Want::of(&airing("The Simpsons", "S02E09", "Itchy & Scratchy & Marge", &[], None)).unwrap();
    let b = Want::of(&airing("the simpsons", "s2e9", "", &[], None)).unwrap();
    assert_eq!(a.key(), b.key(), "a numbered episode is keyed by its numbers");
    let d84 = Want::Film { title: "Dune".into(), year: Some(1984) };
    let d21 = Want::Film { title: "Dune".into(), year: Some(2021) };
    assert_ne!(d84.key(), d21.key());
}

#[test]
fn a_lookup_with_no_servers_finds_nothing() {
    let _g = plx_base::testlock::serial();
    plx_plex::plex::reset_servers_for_test();
    assert_eq!(lookup(&Want::Film { title: "Sintel".into(), year: None }), None);
}
