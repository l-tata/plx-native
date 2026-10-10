use super::*;

const MIN: i64 = 60_000;

fn ep(show: &str, season: i64, index: i64, mins: i64) -> Program {
    Program {
        rk: format!("{show}-{season}-{index}"),
        episode: true,
        title: format!("{show} {season}x{index}"),
        show_title: show.to_owned(),
        show_rk: show.to_owned(),
        season,
        index,
        dur_ms: mins * MIN,
        ..Default::default()
    }
}

fn film(name: &str, mins: i64) -> Program {
    Program { rk: name.to_owned(), title: name.to_owned(), dur_ms: mins * MIN, ..Default::default() }
}

fn mixed() -> Vec<Program> {
    let mut v = Vec::new();
    for i in 1..=6 {
        v.push(ep("A", 1, i, 22));
    }
    for i in 1..=4 {
        v.push(ep("B", 1, i, 44));
    }
    for i in 1..=3 {
        v.push(ep("C", 2, i, 30));
    }
    v.push(film("F1", 100));
    v.push(film("F2", 95));
    v
}

#[test]
fn the_slot_at_any_instant_is_contiguous_with_its_neighbours() {
    for style in Style::ALL {
        let s = Schedule::new(mixed(), style, 42, 1_000_000);
        let mut t = 1_000_000 - 3 * s.pass_ms();
        let mut prev: Option<Slot> = None;
        while t < 1_000_000 + 3 * s.pass_ms() {
            let slot = s.at(t).expect("a non-empty channel always has something on");
            assert!(slot.covers(t), "{style:?} {slot:?} {t}");
            if let Some(p) = prev.filter(|p| p.stop_ms <= t) {
                assert_eq!(p.stop_ms, slot.start_ms, "{style:?}: no gaps, no overlaps");
            }
            prev = Some(slot);
            t = slot.stop_ms;
        }
    }
}

#[test]
fn every_pass_airs_every_programme_exactly_once() {
    for style in Style::ALL {
        let s = Schedule::new(mixed(), style, 7, 0);
        for pass in [-2, 0, 1, 5] {
            let mut order = s.pass_order(pass).to_vec();
            assert_eq!(order.len(), s.programs().len(), "{style:?}");
            order.sort_unstable();
            assert_eq!(order, (0..s.programs().len()).collect::<Vec<_>>(), "{style:?}");
        }
    }
}

#[test]
fn the_same_recipe_is_the_same_timeline_and_a_new_seed_is_another() {
    let a = Schedule::new(mixed(), Style::Random, 99, 5_000);
    let b = Schedule::new(mixed(), Style::Random, 99, 5_000);
    assert_eq!(a.window(0, 24 * 60 * MIN), b.window(0, 24 * 60 * MIN), "two televisions agree");
    let c = a.reseeded(100);
    assert_ne!(a.pass_order(0), c.pass_order(0), "Reshuffle changes the order");
    assert_ne!(a.pass_order(0), a.pass_order(1), "each pass is a fresh order");
}

#[test]
fn a_window_matches_the_slots_at_each_instant_and_covers_the_span() {
    let s = Schedule::new(mixed(), Style::Blocks, 3, 12_345);
    let (from, to) = (100 * MIN, 100 * MIN + 30 * 60 * MIN);
    let w = s.window(from, to);
    assert!(w.first().unwrap().start_ms <= from && w.last().unwrap().stop_ms >= to);
    for pair in w.windows(2) {
        assert_eq!(pair[0].stop_ms, pair[1].start_ms);
    }
    for slot in &w {
        assert_eq!(s.at(slot.start_ms), Some(*slot));
    }
}

#[test]
fn rotation_alternates_shows_and_keeps_each_shows_episodes_in_order() {
    let s = Schedule::new(mixed(), Style::RoundRobin, 11, 0);
    let order = s.pass_order(0);
    let p = s.programs();
    // The first four slots are four different groups (A, B, C and "a film").
    let first: Vec<&str> = order[..4].iter().map(|&i| if p[i].episode { p[i].show_rk.as_str() } else { "film" }).collect();
    let mut uniq = first.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), 4, "{first:?}");
    for show in ["A", "B", "C"] {
        let idx: Vec<i64> = order.iter().filter(|&&i| p[i].show_rk == show).map(|&i| p[i].index).collect();
        let mut sorted = idx.clone();
        sorted.sort_unstable();
        assert_eq!(idx, sorted, "{show} airs in order");
    }
}

#[test]
fn blocks_keep_two_or_three_consecutive_episodes_together() {
    let s = Schedule::new(mixed(), Style::Blocks, 5, 0);
    let order = s.pass_order(0);
    let p = s.programs();
    let mut run = 1;
    for w in order.windows(2) {
        if p[w[0]].episode && p[w[0]].show_rk == p[w[1]].show_rk {
            assert_eq!(p[w[1]].index, p[w[0]].index + 1, "a block is consecutive episodes");
            run += 1;
        } else {
            assert!(run <= 3, "blocks are at most three long");
            run = 1;
        }
    }
}

#[test]
fn random_keeps_the_same_show_off_consecutive_slots_when_it_can() {
    let s = Schedule::new(mixed(), Style::Random, 1234, 0);
    for pass in 0..20 {
        let order = s.pass_order(pass);
        let p = s.programs();
        let repeats = order.windows(2).filter(|w| p[w[0]].episode && p[w[0]].show_rk == p[w[1]].show_rk).count();
        // Six episodes of A among fifteen programmes can always be spread out except at the tail.
        assert!(repeats <= 1, "pass {pass}: {repeats} back-to-back repeats");
    }
}

#[test]
fn in_order_is_the_source_order_every_pass() {
    let s = Schedule::new(mixed(), Style::InOrder, 1, 0);
    assert_eq!(*s.pass_order(0), (0..s.programs().len()).collect::<Vec<_>>());
    assert_eq!(s.pass_order(0), s.pass_order(9));
}

#[test]
fn a_programme_without_a_length_is_left_out_and_an_empty_channel_has_nothing_on() {
    let s = Schedule::new(vec![film("none", 0), film("ok", 10)], Style::Random, 1, 0);
    assert_eq!(s.programs().len(), 1);
    let empty = Schedule::new(vec![film("none", 0)], Style::Random, 1, 0);
    assert!(empty.is_empty() && empty.at(123).is_none() && empty.window(0, 1000).is_empty());
}

#[test]
fn a_random_join_lands_inside_the_first_pass_and_is_stable_for_its_seed() {
    let s = Schedule::new(mixed(), Style::Random, 1, 0);
    let now = 1_700_000_000_000;
    let e = epoch_for_random_join(s.pass_ms(), now, 77);
    assert!(e <= now && now - e < s.pass_ms());
    assert_eq!(e, epoch_for_random_join(s.pass_ms(), now, 77));
    assert_ne!(e, epoch_for_random_join(s.pass_ms(), now, 78));
}

#[test]
fn styles_round_trip_through_their_keys_and_seeds_are_stable() {
    for s in Style::ALL {
        assert_eq!(Style::from_key(s.key()), Some(s));
    }
    assert_eq!(seed_of("abc"), seed_of("abc"));
    assert_ne!(seed_of("abc"), seed_of("abd"));
    assert_eq!(seed_of(""), 0xcbf2_9ce4_8422_2325, "FNV-1a's offset basis: the hash never changes");
}

#[test]
fn a_big_channel_orders_a_pass_quickly() {
    let mut v = Vec::new();
    for show in 0..60 {
        for i in 0..100 {
            v.push(ep(&format!("S{show}"), 1, i, 25));
        }
    }
    for style in Style::ALL {
        let s = Schedule::new(v.clone(), style, 9, 0);
        let t = std::time::Instant::now();
        let w = s.window(0, 48 * 60 * MIN);
        assert!(!w.is_empty());
        // A host debug build; the bound is loose but catches anything quadratic in 6000 programmes.
        assert!(t.elapsed().as_millis() < 2_000, "{style:?} took {:?}", t.elapsed());
    }
}
