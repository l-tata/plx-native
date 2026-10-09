//! The library's further filters in the store: offered by the section, combined into the query,
//! refused where they do not apply, and dropped with the listing type (`browse::filters`).

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

fn offering(browse: &mut TestBrowse, fields: &[(&str, filters::FilterKind)]) {
    browse.state.states[0].filter_defs = Arc::new(fields.iter().map(|(field, kind)| filters::FilterDef {
        field: (*field).into(), title: (*field).into(), kind: *kind,
    }).collect());
}

#[test]
fn further_filters_combine_with_the_genre_and_unwatched_in_one_query() {
    let _g = plx_base::testlock::serial();
    let mut browse = TestBrowse::default();
    seed_one_section(&mut browse);
    offering(&mut browse, &[("year", filters::FilterKind::Values), ("hdr", filters::FilterKind::Switch)]);
    browse.state.states[0].unwatched = true;
    browse.state.states[0].genre = Some(Arc::new(GenreEntry { id: "3".into(), title: "s3".into() }));
    let gen = browse.state.query_gen();
    assert!(browse.state.set_filter("year", Some(("1999".into(), "1999".into()))));
    assert!(browse.state.set_filter("hdr", Some(("1".into(), "HDR".into()))));
    assert_ne!(browse.state.query_gen(), gen, "a filter is a new query");
    let filters = browse.state.states[0].query_filters(SecKind::Movie);
    for pair in [("unwatched", "1"), ("genre", "3"), ("year", "1999"), ("hdr", "1")] {
        assert!(filters.contains(&(pair.0.to_owned(), pair.1.to_owned())), "{pair:?} in {filters:?}");
    }
    assert!(browse.state.set_filter("year", None));
    assert!(!browse.state.states[0].query_filters(SecKind::Movie).iter().any(|(k, _)| k == "year"));
}

#[test]
fn a_field_the_section_does_not_offer_is_refused() {
    let _g = plx_base::testlock::serial();
    let mut browse = TestBrowse::default();
    seed_one_section(&mut browse);
    offering(&mut browse, &[("year", filters::FilterKind::Values)]);
    assert!(!browse.state.set_filter("director", Some(("9".into(), "s9".into()))));
    assert!(browse.state.states[0].more.is_empty());
}
