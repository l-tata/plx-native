use super::*;
use plx_data::livetv::guide::Channel;

const MIN: i64 = 60_000;
const T0: i64 = 1_791_576_000_000;

fn airing(start_min: i64, len_min: i64, cats: &[&str]) -> Airing {
    Airing {
        start_ms: T0 + start_min * MIN,
        stop_ms: T0 + (start_min + len_min) * MIN,
        categories: cats.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    }
}

fn lineup() -> Lineup {
    let ch = |n: &str, airings: Vec<Airing>| Channel { number: n.into(), airings, ..Default::default() };
    Lineup {
        channels: vec![
            ch("1", vec![airing(0, 60, &["Movie"]), airing(60, 60, &["News"])]),
            ch("2", vec![airing(0, 30, &["Series", "Sitcom"]), airing(30, 30, &["Reality"])]),
            ch("3", vec![airing(0, 120, &["Sports event"])]),
            ch("4", vec![airing(180, 60, &["Movie"])]),
            ch("5", vec![airing(0, 30, &["Reality"]), airing(30, 30, &["Cooking"]), airing(60, 30, &["reality"])]),
        ],
        ..Default::default()
    }
}

#[test]
fn a_genre_filter_reads_every_category_and_a_category_filter_ignores_case() {
    let cartoon = airing(0, 30, &["Comedy", "Animation"]);
    assert!(Filter::Genre(Genre::Kids).matches(&cartoon));
    assert!(Filter::Genre(Genre::Comedy).matches(&cartoon));
    assert!(!Filter::Genre(Genre::News).matches(&cartoon));
    assert!(Filter::Category("animation".into()).matches(&cartoon));
    assert!(Filter::All.matches(&Airing::default()));
}

#[test]
fn the_strip_offers_all_the_fixed_four_present_genres_then_frequent_categories() {
    let chips = chips(&lineup());
    assert_eq!(
        chips,
        vec![
            Filter::All,
            Filter::Genre(Genre::Movie),
            Filter::Genre(Genre::Sports),
            Filter::Genre(Genre::Kids),
            Filter::Genre(Genre::News),
            Filter::Genre(Genre::Comedy),
            Filter::Category("Reality".into()),
            Filter::Category("Cooking".into()),
            Filter::Category("Series".into()),
        ],
        "Kids is offered though nothing is for kids; Documentary is not; Reality (3, any case) leads"
    );
}

#[test]
fn the_strip_is_capped() {
    let many: Vec<Airing> = (0..20).map(|i| airing(i, 1, &[&format!("Cat{i:02}")])).collect();
    let l = Lineup { channels: vec![Channel { airings: many, ..Default::default() }], ..Default::default() };
    let chips = chips(&l);
    assert_eq!(chips.len(), MAX_CHIPS);
    assert_eq!(chips[5], Filter::Category("Cat00".into()), "ties are by name");
}

#[test]
fn a_filter_keeps_the_channels_with_a_match_in_the_window() {
    let l = lineup();
    let window = (T0, T0 + 120 * MIN);
    assert_eq!(visible(&l, &Filter::All, window.0, window.1), [0, 1, 2, 3, 4]);
    assert_eq!(visible(&l, &Filter::Genre(Genre::Movie), window.0, window.1), [0], "channel 4's film is later");
    assert_eq!(visible(&l, &Filter::Genre(Genre::Movie), T0 + 150 * MIN, T0 + 270 * MIN), [3]);
    assert_eq!(visible(&l, &Filter::Category("REALITY".into()), window.0, window.1), [1, 4]);
    assert!(visible(&l, &Filter::Genre(Genre::Documentary), window.0, window.1).is_empty());
}

#[test]
fn the_cursor_reseats_on_its_channel_or_the_next_one_shown() {
    assert_eq!(reseat(&[0, 2, 4], Some(2)), 1);
    assert_eq!(reseat(&[0, 2, 4], Some(3)), 2, "the next shown channel");
    assert_eq!(reseat(&[0, 2, 4], Some(9)), 2, "past the end: the last");
    assert_eq!(reseat(&[], Some(1)), 0);
    assert_eq!(reseat(&[5], None), 0);
}

#[test]
fn timing_says_minutes_left_now_and_starts_in_within_the_hour() {
    let a = airing(0, 30, &[]);
    assert_eq!(Timing::of(&a, T0 + 18 * MIN), Timing::Left(12 * MIN));
    assert_eq!(Timing::of(&a, T0 - 25 * MIN), Timing::StartsIn(25 * MIN));
    assert_eq!(Timing::of(&a, T0 - 61 * MIN), Timing::Quiet, "beyond the hour the start time says it");
    assert_eq!(Timing::of(&a, T0 + 30 * MIN), Timing::Quiet, "over");
    let _en = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    assert_eq!(Timing::Left(12 * MIN - 1).text().as_deref(), Some("12 min left"), "rounded up");
    assert_eq!(Timing::StartsIn(30_000).text().as_deref(), Some("Starts in 1 min"), "never in 0 min");
    assert_eq!(Timing::StartsIn(25 * MIN).text().as_deref(), Some("Starts in 25 min"));
    assert_eq!(Timing::Quiet.text(), None);
}

#[test]
fn a_cell_edge_is_its_first_genre_and_nothing_without_one() {
    assert_eq!(genre_edge(&airing(0, 1, &["Series", "Sports"])), Some(theme::GUIDE_GENRE_SPORTS));
    assert_eq!(genre_edge(&airing(0, 1, &["Animation", "Comedy"])), Some(theme::GUIDE_GENRE_KIDS));
    assert_eq!(genre_edge(&airing(0, 1, &["Reality"])), None);
}

#[test]
fn canon_spellings_are_distinct_and_stable() {
    assert_eq!(Filter::All.canon(), "all");
    assert_eq!(Filter::Genre(Genre::News).canon(), "genre:News");
    assert_eq!(Filter::Category("Reality".into()).canon(), Filter::Category("reality".into()).canon());
}
