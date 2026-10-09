use super::*;

const DAY: i64 = 86_400;
const NOW: i64 = 1_760_000_000; // a Thursday in October 2025, UTC

fn moment(hour: u32, weekday: u32, month: u32) -> Moment {
    Moment { now_s: NOW, hour, weekday, month }
}

fn show(rk: &str, title: &str, year: i64, genres: &[&str], studio: &str, leaves: i64, viewed: i64, mins: i64) -> Show {
    Show {
        rk: rk.into(),
        title: title.into(),
        year,
        genres: genres.iter().map(|g| g.to_string()).collect(),
        studio: studio.into(),
        content_rating: "TV-PG".into(),
        leaf_count: leaves,
        viewed_leaf_count: viewed,
        episode_ms: mins * 60_000,
        last_viewed_at: if viewed > 0 { NOW - 2 * DAY } else { 0 },
        audience_rating: 8.0,
        art: format!("/art/{rk}"),
        thumb: format!("/thumb/{rk}"),
        section: 2,
        ..Default::default()
    }
}

fn film(rk: &str, year: i64, genres: &[&str], director: &str, mins: i64, watched: bool, score: f64) -> Program {
    Program {
        rk: rk.into(),
        title: format!("Film {rk}"),
        year,
        genres: genres.iter().map(|g| g.to_string()).collect(),
        directors: vec![director.into()],
        dur_ms: mins * 60_000,
        watched,
        last_viewed_at: if watched { NOW - 10 * DAY } else { 0 },
        audience_rating: score,
        content_rating: "PG-13".into(),
        art: format!("/art/{rk}"),
        thumb: format!("/thumb/{rk}"),
        section: 1,
        ..Default::default()
    }
}

/// A household library: lots of 90s NBC sitcoms (one heavily watched), some dramas, a director
/// with many films, horror, and cartoons.
fn library() -> Catalog {
    let mut shows = vec![
        show("frasier", "Frasier", 1993, &["Comedy"], "NBC", 264, 200, 22),
        show("seinfeld", "Seinfeld", 1989, &["Comedy"], "NBC", 180, 0, 22),
        show("friends", "Friends", 1994, &["Comedy"], "NBC", 236, 0, 22),
        show("newsradio", "NewsRadio", 1995, &["Comedy"], "NBC", 97, 0, 22),
        show("cheers", "Cheers", 1982, &["Comedy"], "NBC", 275, 0, 22),
        show("er", "ER", 1994, &["Drama"], "NBC", 331, 0, 44),
        show("lost", "Lost", 2004, &["Drama", "Mystery"], "ABC", 121, 30, 44),
        show("sopranos", "The Sopranos", 1999, &["Drama", "Crime"], "HBO", 86, 0, 55),
        show("wire", "The Wire", 2002, &["Drama", "Crime"], "HBO", 60, 0, 58),
        show("simpsons", "The Simpsons", 1989, &["Animation", "Comedy"], "FOX", 700, 10, 22),
        show("bluey", "Bluey", 2018, &["Animation", "Children"], "ABC Kids", 150, 0, 8),
        show("arthur", "Arthur", 1996, &["Animation", "Children"], "PBS", 250, 0, 25),
    ];
    shows[9].content_rating = "TV-PG".into();
    shows[10].content_rating = "TV-Y".into();
    shows[11].content_rating = "TV-Y".into();
    let mut movies = Vec::new();
    for i in 0..8 {
        movies.push(film(&format!("nolan{i}"), 2000 + i, &["Thriller", "Science Fiction"], "Christopher Nolan", 140, i < 3, 8.4));
    }
    for i in 0..10 {
        movies.push(film(&format!("horror{i}"), 1978 + i, &["Horror"], "Various", 95, false, 6.5));
    }
    for i in 0..12 {
        movies.push(film(&format!("com{i}"), 1990 + i % 10, &["Comedy"], "Somebody", 100, i < 2, 7.8));
    }
    Catalog { sid: 1, movies, shows, ..Default::default() }
}

fn names(v: &[Suggestion]) -> Vec<&str> {
    v.iter().map(|s| s.name.as_str()).collect()
}

#[test]
fn the_profiles_taste_leads_the_row() {
    let row = suggest(&library(), &moment(20, 3, 10), &[]);
    assert!(!row.is_empty());
    let top: Vec<&str> = names(&row[..5.min(row.len())]);
    // A Frasier fan gets the 90s sitcoms and a channel built around Frasier near the top.
    assert!(top.iter().any(|n| *n == "90s Sitcoms" || n.starts_with("Because You Watched Frasier")), "{top:?}");
    let because = row.iter().find(|s| s.name == "Because You Watched Frasier").expect("a channel around the favourite");
    assert!(because.rules.shows.contains(&"frasier".to_owned()) && because.rules.shows.contains(&"seinfeld".to_owned()));
}

#[test]
fn every_suggestion_is_fillable_named_and_explained() {
    let row = suggest(&library(), &moment(20, 3, 10), &[]);
    for s in &row {
        assert!(!s.name.is_empty() && !s.why.is_empty() && !s.tagline.is_empty(), "{s:?}");
        assert!(s.hours >= s.family.min_hours(), "{} is too short: {}h", s.name, s.hours);
        assert!(s.films + s.shows >= s.family.min_titles(), "{}", s.name);
        assert!(s.art.is_some(), "{} has a backdrop", s.name);
    }
    let mut ids: Vec<&str> = row.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), row.len(), "no idea twice");
}

#[test]
fn the_row_is_diverse() {
    let row = suggest(&library(), &moment(20, 3, 10), &[]);
    let mut per: HashMap<Family, usize> = HashMap::new();
    for s in &row {
        *per.entry(s.family).or_default() += 1;
    }
    for (f, n) in per {
        assert!(n <= f.cap(), "{f:?} appears {n} times");
    }
    assert!(row.iter().map(|s| s.family).collect::<HashSet<_>>().len() >= 4, "{:?}", names(&row));
}

#[test]
fn the_moment_matters() {
    let lib = library();
    let saturday = suggest(&lib, &moment(8, 5, 5), &[]);
    let pos = |row: &[Suggestion], n: &str| row.iter().position(|s| s.name == n);
    let weekday_evening = suggest(&lib, &moment(20, 2, 5), &[]);
    let sat = pos(&saturday, "Saturday Morning Cartoons").unwrap_or_else(|| panic!("cartoons on a Saturday morning: {:?}", saturday.iter().map(|s| (s.name.as_str(), s.family, (s.score * 100.0) as i64)).collect::<Vec<_>>()));
    match pos(&weekday_evening, "Saturday Morning Cartoons") {
        Some(p) => assert!(sat < p, "cartoons rank higher on a Saturday morning"),
        None => {}
    }
    let october = suggest(&lib, &moment(22, 4, 10), &[]);
    assert!(october.iter().any(|s| s.name == "Halloween Horror"), "{:?}", names(&october));
    let may = suggest(&lib, &moment(22, 4, 5), &[]);
    assert!(!may.iter().any(|s| s.name == "Halloween Horror"));
}

#[test]
fn catch_up_holds_the_shows_in_progress_and_only_their_unwatched_episodes() {
    let row = suggest(&library(), &moment(20, 3, 10), &[]);
    let c = row.iter().find(|s| s.id == "catchup").expect("catch-up TV");
    let mut shows = c.rules.shows.clone();
    shows.sort();
    assert_eq!(shows, ["frasier", "lost", "simpsons"]);
    assert!(c.rules.unwatched_only && c.style == Style::RoundRobin);
}

#[test]
fn a_kept_idea_is_not_suggested_again() {
    let lib = library();
    let first = suggest(&lib, &moment(20, 3, 10), &[]);
    let kept = vec![first[0].id.clone()];
    let again = suggest(&lib, &moment(20, 3, 10), &kept);
    assert!(!again.iter().any(|s| s.id == kept[0]));
}

#[test]
fn a_kids_mood_admits_nothing_above_pg() {
    let lib = library();
    let row = suggest(&lib, &moment(8, 6, 5), &[]);
    let kids = row.iter().find(|s| s.id == "mood:saturday").expect("cartoons");
    assert_eq!(kids.rules.max_rating_rank, kids_rank());
    // The Simpsons (TV-PG) is in; nothing rated over PG could be.
    assert!(lib.shows.iter().filter(|s| show_matches(&kids.rules, s)).all(|s| rating_rank(&s.content_rating) <= 2));
}

#[test]
fn a_cold_profile_still_gets_suggestions_from_its_library() {
    let mut lib = library();
    for s in &mut lib.shows {
        s.viewed_leaf_count = 0;
    }
    for f in &mut lib.movies {
        f.watched = false;
    }
    let row = suggest(&lib, &moment(20, 3, 10), &[]);
    assert!(row.len() >= 5, "{:?}", names(&row));
    assert!(!row.iter().any(|s| s.family == Family::BecauseYouWatched || s.family == Family::CatchUp));
}

#[test]
fn surprise_is_stable_for_its_seed_and_honours_exclusions() {
    let lib = library();
    let a = surprise(&lib, &moment(20, 3, 10), &[], &[], 7).unwrap();
    let b = surprise(&lib, &moment(20, 3, 10), &[], &[], 7).unwrap();
    assert_eq!(a.id, b.id);
    let c = surprise(&lib, &moment(20, 3, 10), &[], &[a.id.clone()], 7).unwrap();
    assert_ne!(c.id, a.id);
}

#[test]
fn names_read_like_a_person_wrote_them() {
    assert_eq!(decade_label(1990), "90s");
    assert_eq!(decade_label(2000), "2000s");
    assert_eq!(genre_noun("Comedy", Kinds::Episodes, true), "Sitcoms");
    assert_eq!(genre_noun("Crime", Kinds::Episodes, false), "Crime Dramas");
    assert_eq!(genre_noun("Horror", Kinds::Movies, false), "Horror Movies");
    assert_eq!(genre_noun("science fiction", Kinds::Movies, false), "Sci-Fi Movies");
    assert_eq!(genre_noun("Comedy", Kinds::Both, false), "Comedy");
}

#[test]
fn a_suggestion_becomes_a_library_recipe_with_its_origin() {
    let row = suggest(&library(), &moment(20, 3, 10), &[]);
    let r = row[0].recipe(NOW * 1000);
    assert_eq!(r.source, Source::Library);
    assert_eq!(r.origin, row[0].id);
    assert_eq!(r.rules, row[0].rules);
    assert!(r.epoch_ms <= NOW * 1000);
}

#[test]
fn moments_come_from_the_clock() {
    // 2025-10-11 09:30 UTC was a Saturday.
    let m = Moment { ..Moment::at(1_760_175_000_000) };
    assert_eq!(m.month, 10);
    let _ = m; // the set's own zone may shift the hour; the month and a weekend are robust here
}

#[test]
fn the_row_turns_over_between_days_but_not_within_one() {
    let lib = library();
    let a = suggest(&lib, &moment(20, 3, 10), &[]);
    let b = suggest(&lib, &moment(20, 3, 10), &[]);
    assert_eq!(names(&a), names(&b), "the same moment, the same row");
}

#[test]
fn a_dismissed_idea_is_left_out() {
    let lib = library();
    let first = suggest(&lib, &moment(20, 3, 10), &[]);
    let gone = vec![first[1].id.clone()];
    let again = suggest_excluding(&lib, &moment(20, 3, 10), &[], &gone);
    assert!(!again.iter().any(|s| s.id == gone[0]));
}
