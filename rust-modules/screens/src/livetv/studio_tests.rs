use super::*;
use plx_data::livetv::LiveTvState;
use plx_data::vchannel::recipe::{Kinds, Recipe};
use plx_data::vchannel::catalog::Estimate;
use plx_machine::machine::Key;

const NOW: i64 = 1_760_000_000_000;

fn idea(id: &str) -> Suggestion {
    Suggestion { id: id.into(), name: format!("Idea {id}"), why: "Because".into(), hours: 40.0, ..Default::default() }
}

fn programme(rk: &str, show: &str) -> Program {
    Program { rk: rk.into(), title: format!("Ep {rk}"), show_title: show.into(), episode: true, season: 1, index: 2, dur_ms: 30 * 60_000, ..Default::default() }
}

fn channel(playlist: &str, number: u32) -> VChannel {
    let recipe = Recipe { name: format!("Ch {number}"), number, seed: 7, epoch_ms: NOW - 3_600_000, ..Default::default() };
    let schedule = Schedule::new(vec![programme("1", "Frasier"), programme("2", "Cheers")], recipe.style, recipe.seed, recipe.epoch_ms);
    VChannel { playlist: playlist.into(), recipe, composite: String::new(), schedule: Some(schedule), built_at_ms: Some(NOW), failed: false }
}

fn state(ideas: &[&str], channels: Vec<VChannel>) -> LiveTvState {
    let mut s = LiveTvState::default();
    s.install_virtuals_for_test(channels, ideas.iter().map(|i| idea(i)).collect(), NOW);
    s
}

#[test]
fn the_rows_hold_the_suggestions_then_surprise_and_the_kept_channels() {
    let s = state(&["a", "b"], vec![channel("p1", 900)]);
    let rows = rows(s.view(), None);
    let ids: Vec<String> = rows[ROW_SUGGESTED].iter().map(|c| c.id()).collect();
    assert_eq!(ids, ["a", "b", SURPRISE_ID]);
    assert_eq!(rows[ROW_YOURS].iter().map(|c| c.id()).collect::<Vec<_>>(), ["pl:p1"]);
    assert!(s.view().configured(), "suggestions alone put Live TV on the strip");
}

#[test]
fn ok_reaches_the_actions_and_back_returns_then_closes() {
    let s = state(&["a"], Vec::new());
    let mut st = Studio::default();
    let mut out = Vec::new();
    assert!(st.key(Key::Ok, s.view(), NOW, &mut out));
    assert_eq!(st.zone, Zone::Actions);
    assert!(st.key(Key::Right, s.view(), NOW, &mut out));
    assert_eq!(st.act, 1);
    assert!(!st.key(Key::Up, s.view(), NOW, &mut out), "UP off the actions is the top strip's");
    st.key(Key::Back, s.view(), NOW, &mut out);
    assert_eq!(st.zone, Zone::Cards);
    st.key(Key::Back, s.view(), NOW, &mut out);
    assert_eq!(out, [Out::Close]);
}

#[test]
fn a_preview_is_asked_once_focus_rests_and_reshuffle_asks_again_with_a_new_seed() {
    let s = state(&["a", "b"], Vec::new());
    let mut st = Studio::default();
    let mut out = Vec::new();
    st.tick(s.view(), 0, NOW, true, &mut out);
    st.tick(s.view(), PREVIEW_DWELL_MS - 1, NOW, true, &mut out);
    assert!(out.is_empty(), "not before the dwell");
    st.tick(s.view(), PREVIEW_DWELL_MS, NOW, true, &mut out);
    let Some(Out::Store(VCmd::Preview { token: t1, recipe: r1 })) = out.pop() else { panic!("{out:?}") };
    assert_eq!(r1.origin, "a");
    st.tick(s.view(), PREVIEW_DWELL_MS * 3, NOW, true, &mut out);
    assert!(out.is_empty(), "asked once");
    st.zone = Zone::Actions;
    st.act = 1; // Reshuffle
    st.activate(s.view(), NOW + 1, &mut out);
    let Some(Out::Store(VCmd::Preview { token: t2, recipe: r2 })) = out.pop() else { panic!() };
    assert!(t2 > t1);
    assert_ne!(r2.seed, r1.seed);
    assert_eq!((r2.epoch_ms, r2.style), (r1.epoch_ms, r1.style), "a reshuffle keeps the start and the order style");
    st.act = 2; // Order
    st.activate(s.view(), NOW + 2, &mut out);
    let Some(Out::Store(VCmd::Preview { recipe: r3, .. })) = out.pop() else { panic!() };
    assert_eq!(r3.style, next_style(r1.style));
    assert_eq!(r3.seed, r2.seed);
}

#[test]
fn keep_uses_the_previews_token_and_is_not_sent_twice() {
    let s = state(&["a"], Vec::new());
    let mut st = Studio::default();
    let mut out = Vec::new();
    st.tick(s.view(), 0, NOW, true, &mut out);
    st.tick(s.view(), PREVIEW_DWELL_MS, NOW, true, &mut out);
    let Some(Out::Store(VCmd::Preview { token, .. })) = out.pop() else { panic!() };
    st.zone = Zone::Actions;
    st.act = 0;
    st.activate(s.view(), NOW, &mut out);
    assert!(matches!(out.pop(), Some(Out::Store(VCmd::Keep { token: k, .. })) if k == token));
    st.activate(s.view(), NOW, &mut out);
    assert!(out.is_empty(), "one Keep in flight");
}

#[test]
fn a_kept_channel_writes_its_changes_and_delete_asks_twice() {
    let s = state(&[], vec![channel("p1", 900)]);
    let mut st = Studio::default();
    st.clamp(&rows(s.view(), None));
    assert_eq!(st.row, ROW_YOURS, "an empty suggestion row hands the cursor to the kept channels");
    let mut out = Vec::new();
    st.zone = Zone::Actions;
    st.act = 0;
    st.activate(s.view(), NOW, &mut out);
    assert_eq!(out.pop(), Some(Out::Tune("900".into())));
    st.act = 2;
    st.activate(s.view(), NOW, &mut out);
    assert!(matches!(out.pop(), Some(Out::Store(VCmd::Update { ref playlist, ref recipe, rebuild: false })) if playlist == "p1" && recipe.style == Style::RoundRobin));
    st.act = 3;
    st.activate(s.view(), NOW, &mut out);
    assert!(out.is_empty() && st.confirm_delete.is_some(), "the first press asks");
    st.activate(s.view(), NOW, &mut out);
    assert_eq!(out.pop(), Some(Out::Store(VCmd::Delete { playlist: "p1".into() })));
}

#[test]
fn not_interested_dismisses_and_surprise_draws() {
    let s = state(&["a", "b"], Vec::new());
    let mut st = Studio::default();
    let mut out = Vec::new();
    st.zone = Zone::Actions;
    st.act = acts(&rows(s.view(), None)[ROW_SUGGESTED][0]).iter().position(|a| *a == Act::NotInterested).unwrap();
    st.activate(s.view(), NOW, &mut out);
    assert_eq!(out.pop(), Some(Out::Store(VCmd::Dismiss { id: "a".into() })));
    st.col[ROW_SUGGESTED] = 2; // Surprise me
    st.zone = Zone::Actions;
    st.act = 0;
    st.activate(s.view(), NOW, &mut out);
    assert!(matches!(out.pop(), Some(Out::Store(VCmd::Surprise { .. }))));
}

#[test]
fn a_wanted_card_is_seated_when_it_exists() {
    let s = state(&["a", "b"], vec![channel("p1", 900)]);
    let mut st = Studio::default();
    st.open_on(Some("b".into()), s.view());
    st.clamp(&rows(s.view(), None));
    assert_eq!((st.row, st.col[ROW_SUGGESTED]), (ROW_SUGGESTED, 1));
    st.want = Some("pl:p1".into());
    st.clamp(&rows(s.view(), None));
    assert_eq!(st.row, ROW_YOURS);
    st.want = Some("pl:later".into());
    st.clamp(&rows(s.view(), None));
    assert_eq!(st.want.as_deref(), Some("pl:later"), "kept until the channel lands");
}

#[test]
fn programmes_read_as_a_viewer_names_them() {
    assert_eq!(programme_line(&programme("1", "Frasier")), "Frasier \u{b7} S1 \u{b7} E2 \u{b7} Ep 1");
    let film = Program { title: "Heat".into(), year: 1995, ..Default::default() };
    assert_eq!(programme_line(&film), "Heat (1995)");
    assert_eq!(next_style(Style::InOrder), Style::Random);
}

#[test]
fn reshuffle_always_puts_something_else_on_now() {
    let programmes: Vec<Program> = (0..6).map(|i| programme(&i.to_string(), &format!("Show {i}"))).collect();
    let schedule = Schedule::new(programmes, Style::Random, 11, NOW - 5 * 3_600_000);
    for t in 0..20 {
        let wall = NOW + t * 977_000;
        let before = schedule.at(wall).unwrap().program;
        let seed = fresh_seed(Some(&schedule), 11, wall);
        assert_ne!(schedule.reseeded(seed).at(wall).unwrap().program, before, "at {wall}");
    }
    assert_eq!(fresh_seed(None, 11, NOW), fresh_seed(None, 11, NOW), "deterministic without a timeline");
}

#[test]
fn edit_opens_the_options_and_each_change_previews_the_draft_again() {
    let s = state(&["a"], Vec::new());
    let mut st = Studio::default();
    let mut out = Vec::new();
    st.zone = Zone::Actions;
    st.act = acts(&rows(s.view(), None)[ROW_SUGGESTED][0]).iter().position(|a| *a == Act::Edit).unwrap();
    st.activate(s.view(), NOW, &mut out);
    assert_eq!(st.zone, Zone::Edit);
    assert!(out.is_empty(), "opening the options changes nothing");
    // Kinds: films and shows -> films only.
    assert!(st.key(Key::Ok, s.view(), NOW, &mut out));
    let Some(Out::Store(VCmd::Preview { recipe, .. })) = out.pop() else { panic!("{out:?}") };
    assert_eq!(recipe.rules.kinds, Kinds::Movies);
    // Unwatched only, on.
    st.key(Key::Right, s.view(), NOW, &mut out);
    st.key(Key::Ok, s.view(), NOW, &mut out);
    let Some(Out::Store(VCmd::Preview { recipe, .. })) = out.pop() else { panic!() };
    assert!(recipe.rules.unwatched_only && recipe.rules.kinds == Kinds::Movies, "options add up");
    // Rating: any -> G.
    st.key(Key::Right, s.view(), NOW, &mut out);
    st.key(Key::Ok, s.view(), NOW, &mut out);
    let Some(Out::Store(VCmd::Preview { recipe, .. })) = out.pop() else { panic!() };
    assert_eq!(recipe.rules.max_rating_rank, 1);
    assert_eq!(edit_label(EditItem::Rating, &recipe), plx_platform::i18n::msg::livetv_studio_rating_up_to("G"));
    // Done returns to the actions, on Edit; Keep then keeps the edited draft.
    st.key(Key::Right, s.view(), NOW, &mut out);
    st.key(Key::Ok, s.view(), NOW, &mut out);
    assert_eq!((st.zone, acts(&rows(s.view(), None)[ROW_SUGGESTED][0])[st.act]), (Zone::Actions, Act::Edit));
    st.act = 0;
    st.activate(s.view(), NOW, &mut out);
    let Some(Out::Store(VCmd::Keep { recipe, .. })) = out.pop() else { panic!() };
    assert!(recipe.rules.unwatched_only && recipe.rules.max_rating_rank == 1);
}

#[test]
fn options_cycle_and_the_count_reads_naturally() {
    assert_eq!(next_kinds(next_kinds(next_kinds(Kinds::Both))), Kinds::Both);
    assert_eq!((next_rating(0), next_rating(2), next_rating(3)), (1, 3, 0));
    let line = estimate_line(&Estimate { films: 1, shows: 0, programmes: 1, hours: 2.4 });
    assert_eq!(line, format!("{} \u{b7} {}", plx_platform::i18n::msg::browse_person_films(1), plx_platform::i18n::msg::livetv_studio_hours(2)));
    assert_eq!(estimate_line(&Estimate::default()), "");
}

fn catalog_state() -> LiveTvState {
    use plx_data::vchannel::catalog::Catalog;
    let film = |rk: &str, year: i64, genre: &str| Program { rk: rk.into(), title: rk.into(), year, genres: vec![genre.into()], dur_ms: 90 * 60_000, ..Default::default() };
    let cat = Catalog { sid: 0, movies: vec![film("a", 1985, "Comedy"), film("b", 1992, "Comedy"), film("c", 1994, "Horror"), film("d", 1971, "Horror")], ..Default::default() };
    let mut s = state(&["x"], Vec::new());
    s.install_catalog_for_test(cat);
    s
}

#[test]
fn new_channel_starts_from_the_library_and_names_itself_from_its_options() {
    let s = catalog_state();
    let mut st = Studio::default();
    let mut out = Vec::new();
    let r = rows(s.view(), None);
    assert!(matches!(r[ROW_YOURS].last(), Some(Card::Create)), "New Channel closes Your Channels once the library is read");
    st.row = ROW_YOURS;
    st.col[ROW_YOURS] = r[ROW_YOURS].len() - 1;
    assert!(st.key(Key::Ok, s.view(), NOW, &mut out));
    assert_eq!(st.zone, Zone::Edit, "its options open at once");
    let made = st.made().cloned().expect("a channel being made");
    assert_eq!(made.name, "Everything");
    assert_eq!(st.focus(&rows(s.view(), st.made()) ).map(|c| c.id()), Some(made.id.clone()), "it is the focused first card");
    // Genre: the library's most-held genre first.
    st.key(Key::Ok, s.view(), NOW, &mut out);
    let Some(Out::Store(VCmd::Preview { recipe, .. })) = out.pop() else { panic!("{out:?}") };
    assert_eq!((recipe.rules.genres.clone(), recipe.name.as_str()), (vec!["Comedy".to_owned()], "Comedy"));
    // Decade: the oldest the library holds.
    st.key(Key::Right, s.view(), NOW, &mut out);
    st.key(Key::Ok, s.view(), NOW, &mut out);
    let Some(Out::Store(VCmd::Preview { recipe, .. })) = out.pop() else { panic!() };
    assert_eq!((recipe.rules.year_from, recipe.rules.year_to, recipe.name.as_str()), (1980, 1989, "80s Comedy"));
    assert_eq!(st.made().unwrap().name, "80s Comedy", "the card follows the name");
    let line = estimate_line(st.estimate_of(st.made().unwrap()).expect("a live count"));
    assert_eq!(line, format!("{} \u{b7} {}", plx_platform::i18n::msg::browse_person_films(1), plx_platform::i18n::msg::livetv_studio_hours(2)));
}

#[test]
fn a_made_channel_keeps_its_source_and_discard_drops_it() {
    use plx_data::vchannel::recipe::Source;
    let s = state(&["x"], Vec::new());
    let mut st = Studio::default();
    let mut out = Vec::new();
    st.open_make(Recipe::made(Source::Show { rk: "7".into() }, "Frasier", "From Frasier", NOW), "From Frasier".into(), s.view());
    let made = st.made().cloned().unwrap();
    st.clamp(&rows(s.view(), Some(&made)));
    assert_eq!((st.row, st.col[ROW_SUGGESTED]), (ROW_SUGGESTED, 0), "the made channel leads the row, focused");
    assert_eq!(acts(&Card::Idea(&made)).last(), Some(&Act::Discard), "it is discarded, not dismissed");
    st.zone = Zone::Actions;
    st.act = 0;
    st.activate(s.view(), NOW, &mut out);
    let Some(Out::Store(VCmd::Keep { recipe, .. })) = out.pop() else { panic!("{out:?}") };
    assert_eq!((recipe.source.clone(), recipe.style, recipe.name.as_str()), (Source::Show { rk: "7".into() }, Style::InOrder, "Frasier"));
    st.keeping = None;
    st.act = acts(&Card::Idea(&made)).iter().position(|a| *a == Act::Discard).unwrap();
    st.activate(s.view(), NOW, &mut out);
    assert!(st.made().is_none() && out.is_empty(), "discarding writes nothing");
    assert_eq!(edit_items(&recipe, true), &EDIT_ITEMS, "a show's channel narrows; only a library channel picks genre and decade");
}

#[test]
fn an_option_only_offers_values_that_still_air_something() {
    let s = catalog_state();
    let cat = s.view().virtuals().catalog().unwrap().clone();
    let comedy = plx_data::vchannel::recipe::Rules { genres: vec!["Comedy".into()], ..Default::default() };
    assert_eq!(decade_choices(&cat, &comedy), [1980, 1990], "no 1970s: the library's only 70s film is horror");
    let seventies = plx_data::vchannel::recipe::Rules { year_from: 1970, year_to: 1979, ..Default::default() };
    assert_eq!(genre_choices(&cat, &seventies), ["Horror"]);
    assert_eq!(library_decades(&cat), [1970, 1980, 1990]);
}
