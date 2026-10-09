use super::*;
use plx_data::livetv::LiveTvState;
use plx_data::vchannel::recipe::Recipe;
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
    let rows = rows(s.view());
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
    st.clamp(&rows(s.view()));
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
    st.act = 3;
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
    st.clamp(&rows(s.view()));
    assert_eq!((st.row, st.col[ROW_SUGGESTED]), (ROW_SUGGESTED, 1));
    st.want = Some("pl:p1".into());
    st.clamp(&rows(s.view()));
    assert_eq!(st.row, ROW_YOURS);
    st.want = Some("pl:later".into());
    st.clamp(&rows(s.view()));
    assert_eq!(st.want.as_deref(), Some("pl:later"), "kept until the channel lands");
}

#[test]
fn programmes_read_as_a_viewer_names_them() {
    assert_eq!(programme_line(&programme("1", "Frasier")), "Frasier \u{b7} S1 \u{b7} E2 \u{b7} Ep 1");
    let film = Program { title: "Heat".into(), year: 1995, ..Default::default() };
    assert_eq!(programme_line(&film), "Heat (1995)");
    assert_eq!(next_style(Style::InOrder), Style::Random);
}
