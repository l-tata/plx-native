use super::*;
use crate::vchannel::recipe::Kinds;

fn film(rk: &str, year: i64, genres: &[&str], rating: &str, mins: i64) -> Program {
    Program {
        rk: rk.into(),
        title: rk.into(),
        year,
        genres: genres.iter().map(|g| g.to_string()).collect(),
        content_rating: rating.into(),
        dur_ms: mins * 60_000,
        section: 1,
        ..Default::default()
    }
}

fn show(rk: &str, year: i64, genres: &[&str], studio: &str, leaves: i64, viewed: i64) -> Show {
    Show {
        rk: rk.into(),
        title: rk.into(),
        year,
        genres: genres.iter().map(|g| g.to_string()).collect(),
        studio: studio.into(),
        content_rating: "TV-PG".into(),
        leaf_count: leaves,
        viewed_leaf_count: viewed,
        episode_ms: 22 * 60_000,
        section: 2,
        ..Default::default()
    }
}

fn cat() -> Catalog {
    Catalog {
        sid: 1,
        sections: vec![Section { key: 1, movies: true, title: "Movies".into() }, Section { key: 2, movies: false, title: "TV".into() }],
        movies: vec![
            film("heat", 1995, &["Crime", "Action"], "R", 170),
            film("toy", 1995, &["Animation", "Comedy"], "G", 81),
            film("up", 2009, &["Animation"], "PG", 96),
        ],
        shows: vec![
            show("seinfeld", 1989, &["Comedy"], "NBC", 180, 180),
            show("frasier", 1993, &["Comedy"], "NBC", 264, 20),
            show("lost", 2004, &["Drama", "Mystery"], "ABC", 121, 0),
        ],
    }
}

#[test]
fn genres_years_and_kinds_narrow_films_and_shows() {
    let c = cat();
    let r = Rules { genres: vec!["comedy".into()], year_from: 1990, year_to: 1999, ..Default::default() };
    let films: Vec<&str> = c.films(&r).map(|p| p.rk.as_str()).collect();
    let shows: Vec<&str> = c.shows_matching(&r).map(|s| s.rk.as_str()).collect();
    assert_eq!(films, ["toy"]);
    assert_eq!(shows, ["frasier"], "Seinfeld began in 1989");
    let eps_only = Rules { kinds: Kinds::Episodes, ..r.clone() };
    assert_eq!(c.films(&eps_only).count(), 0);
}

#[test]
fn ratings_studios_and_unwatched() {
    let c = cat();
    let kids = Rules { max_rating_rank: 2, ..Default::default() };
    let mut films: Vec<&str> = c.films(&kids).map(|p| p.rk.as_str()).collect();
    films.sort_unstable();
    assert_eq!(films, ["toy", "up"]);
    let nbc = Rules { studios: vec!["nbc".into()], unwatched_only: true, ..Default::default() };
    let shows: Vec<&str> = c.shows_matching(&nbc).map(|s| s.rk.as_str()).collect();
    assert_eq!(shows, ["frasier"], "Seinfeld is all watched");
}

#[test]
fn a_shows_list_picks_only_those_shows_and_no_films() {
    let c = cat();
    let r = Rules { shows: vec!["lost".into()], ..Default::default() };
    assert_eq!(c.films(&r).count(), 0);
    assert_eq!(c.shows_matching(&r).map(|s| s.rk.as_str()).collect::<Vec<_>>(), ["lost"]);
}

#[test]
fn the_estimate_adds_films_and_episodes() {
    let c = cat();
    let r = Rules { studios: vec!["NBC".into()], ..Default::default() };
    let e = c.estimate(&r);
    assert_eq!((e.films, e.shows, e.programmes), (0, 2, 444));
    assert!((e.hours - 444.0 * 22.0 / 60.0).abs() < 0.01, "{e:?}");
}

#[test]
fn an_episode_inherits_its_shows_facts() {
    let s = show("frasier", 1993, &["Comedy"], "NBC", 264, 20);
    let m = Metadata { kind: "episode".into(), rating_key: "e1".into(), title: "Pilot".into(), duration: 22 * 60_000, ..Default::default() };
    let p = programme_of(&m, 1, Some(&s));
    assert_eq!((p.show_rk.as_str(), p.studio.as_str(), p.year), ("frasier", "NBC", 1993));
    assert_eq!(p.genres, ["Comedy"]);
}

#[test]
fn a_programme_carries_its_part_so_a_tune_plays_it_directly() {
    let media = plx_plex::plex::Media {
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        part: vec![plx_plex::plex::MediaPart { key: "/library/parts/7/file.mkv".into(), ..Default::default() }],
        ..Default::default()
    };
    let m = Metadata { kind: "movie".into(), rating_key: "f1".into(), media: vec![media], ..Default::default() };
    let p = programme_of(&m, 1, None);
    assert_eq!((p.part.as_str(), p.vcodec.as_str(), p.acodec.as_str()), ("/library/parts/7/file.mkv", "h264", "aac"));
    assert_eq!(programme_of(&Metadata::default(), 1, None).part, "", "no media, no part");
}

#[test]
fn excluded_items_and_lengths_narrow_any_source() {
    let r = Rules { min_minutes: 90, exclude: vec!["heat".into()], ..Default::default() };
    let c = cat();
    let kept: Vec<&str> = c.movies.iter().filter(|p| narrows(&r, p)).map(|p| p.rk.as_str()).collect();
    assert_eq!(kept, ["up"]);
}
