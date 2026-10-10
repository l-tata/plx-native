//! **What the profile likes**, read from its own view state in the catalog: every film it watched
//! and every show it has been watching, weighted by how much and how lately, broken down into the
//! features a channel can be built from — genres, decades, networks and studios, people,
//! collections and labels.
//!
//! The view state is the PROFILE's (PMS answers `viewCount`, `viewedLeafCount` and `lastViewedAt`
//! per user on every listing), so nothing here reads the household's history or needs the owner.
//! Pure: [`Taste::of`] is a function of the catalog and the instant.

use std::collections::HashMap;

use super::catalog::{Catalog, Show};
use super::schedule::Program;

/// How fast a viewing fades: its weight halves every this many days.
const HALF_LIFE_DAYS: f64 = 120.0;

/// A feature a channel can be built from, normalised for comparison.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    Genre(String),
    Decade(i64),
    Network(String),
    Person(String),
    Collection(String),
    Label(String),
}

/// The profile's affinity for each feature, 0..=1 within its kind, and how much it watches at all.
#[derive(Clone, Debug, Default)]
pub struct Taste {
    pub affinity: HashMap<Feature, f64>,
    /// The sum of every title's engagement (0 for a profile that has watched nothing here).
    pub engagement: f64,
    /// Per show / film ratingKey, how engaged the profile is (0..=1, recency included).
    pub per_title: HashMap<String, f64>,
    /// Films' share of what is watched (0..=1); 0.5 when nothing is.
    pub film_share: f64,
}

pub fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

pub fn decade_of(year: i64) -> Option<i64> {
    (year >= 1900).then_some(year / 10 * 10)
}

/// Recency weight of a viewing `last_viewed_at` (unix seconds) at `now_s`: 1 for today, halving
/// every [`HALF_LIFE_DAYS`].
pub fn recency(last_viewed_at: i64, now_s: i64) -> f64 {
    if last_viewed_at <= 0 {
        return 0.35; // watched, date unknown: count it, modestly
    }
    let days = ((now_s - last_viewed_at).max(0)) as f64 / 86_400.0;
    halvings(days / HALF_LIFE_DAYS)
}

/// `0.5^h`, exact at every whole `h` and straight between them: no transcendental function, so
/// two televisions weigh the same viewing identically (`ci/check-deps.sh`, the libm gate).
fn halvings(h: f64) -> f64 {
    let h = h.max(0.0);
    let whole = h.floor();
    if whole >= 60.0 {
        return 0.0;
    }
    let base = 1.0 / (1u64 << whole as u32) as f64;
    base * (1.0 - 0.5 * (h - whole))
}

/// How deep into a show `episodes` watched is, 0..=1: rising quickly over the first episodes and
/// whole at about fifty — a rational curve rather than a logarithm, for the reason [`halvings`]
/// gives.
fn depth(episodes: i64) -> f64 {
    const K: f64 = 12.0;
    let v = episodes.max(0) as f64;
    (v * (50.0 + K) / (50.0 * (v + K))).min(1.0)
}

/// How engaged the profile is with a film (0 when unwatched).
pub fn film_engagement(p: &Program, now_s: i64) -> f64 {
    if !p.watched {
        return 0.0;
    }
    0.4 + 0.6 * recency(p.last_viewed_at, now_s)
}

/// How engaged the profile is with a show: how far through it, how many episodes in, how lately.
pub fn show_engagement(s: &Show, now_s: i64) -> f64 {
    if s.viewed_leaf_count <= 0 {
        return 0.0;
    }
    let completion = (s.viewed_leaf_count as f64 / s.leaf_count.max(1) as f64).min(1.0);
    (0.5 * completion + 0.5 * depth(s.viewed_leaf_count)) * (0.4 + 0.6 * recency(s.last_viewed_at, now_s))
}

/// A film's features.
pub fn film_features(p: &Program) -> Vec<Feature> {
    let mut f: Vec<Feature> = p.genres.iter().map(|g| Feature::Genre(norm(g))).collect();
    f.extend(decade_of(p.year).map(Feature::Decade));
    if !p.studio.is_empty() {
        f.push(Feature::Network(norm(&p.studio)));
    }
    f.extend(p.directors.iter().chain(p.actors.iter().take(3)).map(|n| Feature::Person(norm(n))));
    f.extend(p.collections.iter().map(|c| Feature::Collection(norm(c))));
    f.extend(p.labels.iter().map(|l| Feature::Label(norm(l))));
    f
}

/// A show's features.
pub fn show_features(s: &Show) -> Vec<Feature> {
    let mut f: Vec<Feature> = s.genres.iter().map(|g| Feature::Genre(norm(g))).collect();
    f.extend(decade_of(s.year).map(Feature::Decade));
    if !s.studio.is_empty() {
        f.push(Feature::Network(norm(&s.studio)));
    }
    f.extend(s.actors.iter().take(3).map(|n| Feature::Person(norm(n))));
    f.extend(s.collections.iter().map(|c| Feature::Collection(norm(c))));
    f.extend(s.labels.iter().map(|l| Feature::Label(norm(l))));
    f
}

fn kind_of(f: &Feature) -> u8 {
    match f {
        Feature::Genre(_) => 0,
        Feature::Decade(_) => 1,
        Feature::Network(_) => 2,
        Feature::Person(_) => 3,
        Feature::Collection(_) => 4,
        Feature::Label(_) => 5,
    }
}

impl Taste {
    pub fn of(cat: &Catalog, now_s: i64) -> Taste {
        let mut raw: HashMap<Feature, f64> = HashMap::new();
        let mut t = Taste::default();
        let (mut film_e, mut show_e) = (0.0, 0.0);
        for p in &cat.movies {
            let e = film_engagement(p, now_s);
            if e <= 0.0 {
                continue;
            }
            film_e += e;
            t.per_title.insert(p.rk.clone(), e);
            for f in film_features(p) {
                *raw.entry(f).or_default() += e;
            }
        }
        for s in &cat.shows {
            let e = show_engagement(s, now_s);
            if e <= 0.0 {
                continue;
            }
            // A show is many evenings of viewing: it weighs more than one film.
            let w = e * 2.5;
            show_e += w;
            t.per_title.insert(s.rk.clone(), e);
            for f in show_features(s) {
                *raw.entry(f).or_default() += w;
            }
        }
        t.engagement = film_e + show_e;
        t.film_share = if t.engagement > 0.0 { film_e / t.engagement } else { 0.5 };
        // Normalise within each kind, so a genre is compared with genres and a person with people.
        let mut max = [0f64; 6];
        for (f, v) in &raw {
            let k = kind_of(f) as usize;
            max[k] = max[k].max(*v);
        }
        for (f, v) in raw {
            let m = max[kind_of(&f) as usize];
            if m > 0.0 {
                t.affinity.insert(f, v / m);
            }
        }
        t
    }

    pub fn of_feature(&self, f: &Feature) -> f64 {
        self.affinity.get(f).copied().unwrap_or(0.0)
    }

    pub fn cold(&self) -> bool {
        self.engagement < 0.5
    }

    /// The profile's top genres, strongest first.
    pub fn top_genres(&self, n: usize) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = self
            .affinity
            .iter()
            .filter_map(|(f, a)| match f {
                Feature::Genre(g) => Some((g.clone(), *a)),
                _ => None,
            })
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// How well a title's features fit the profile, 0..=1: genres weigh most, then era and
    /// network, then people and collections.
    pub fn fit(&self, features: &[Feature]) -> f64 {
        let mut by_kind = [(0.0f64, 0usize); 6];
        for f in features {
            let k = kind_of(f) as usize;
            let a = self.of_feature(f);
            by_kind[k].0 = by_kind[k].0.max(a);
            by_kind[k].1 += 1;
        }
        const W: [f64; 6] = [0.5, 0.15, 0.15, 0.1, 0.06, 0.04];
        let mut score = 0.0;
        let mut weight = 0.0;
        for k in 0..6 {
            if by_kind[k].1 > 0 {
                score += W[k] * by_kind[k].0;
                weight += W[k];
            }
        }
        if weight > 0.0 { score / weight } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    const NOW: i64 = 1_760_000_000;

    fn cat() -> Catalog {
        Catalog {
            movies: vec![
                Program { rk: "f1".into(), genres: vec!["Comedy".into()], year: 1995, watched: true, last_viewed_at: NOW - DAY, ..Default::default() },
                Program { rk: "f2".into(), genres: vec!["Horror".into()], year: 1980, watched: true, last_viewed_at: NOW - 900 * DAY, ..Default::default() },
                Program { rk: "f3".into(), genres: vec!["Drama".into()], year: 2010, ..Default::default() },
            ],
            shows: vec![Show {
                rk: "s1".into(),
                genres: vec!["Comedy".into()],
                year: 1993,
                studio: "NBC".into(),
                leaf_count: 200,
                viewed_leaf_count: 150,
                last_viewed_at: NOW - 3 * DAY,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn recent_and_deep_viewing_weighs_most() {
        let t = Taste::of(&cat(), NOW);
        assert_eq!(t.of_feature(&Feature::Genre("comedy".into())), 1.0, "comedy is the profile's genre");
        assert!(t.of_feature(&Feature::Genre("horror".into())) < 0.3, "a film from years ago fades");
        assert_eq!(t.of_feature(&Feature::Genre("drama".into())), 0.0, "unwatched counts for nothing");
        assert!(t.of_feature(&Feature::Decade(1990)) > t.of_feature(&Feature::Decade(1980)));
        assert_eq!(t.top_genres(1)[0].0, "comedy");
        assert!(!t.cold());
    }

    #[test]
    fn recency_halves_on_schedule() {
        assert!((recency(NOW, NOW) - 1.0).abs() < 1e-9);
        assert!((recency(NOW - 120 * DAY, NOW) - 0.5).abs() < 1e-6);
        assert!((recency(NOW - 240 * DAY, NOW) - 0.25).abs() < 1e-6);
        let mid = recency(NOW - 60 * DAY, NOW);
        assert!(mid > 0.5 && mid < 1.0, "{mid}");
        assert_eq!(depth(0), 0.0);
        assert!(depth(5) < depth(20) && depth(60) == 1.0);
    }

    #[test]
    fn fit_prefers_titles_like_what_is_watched() {
        let t = Taste::of(&cat(), NOW);
        let comedy90s = vec![Feature::Genre("comedy".into()), Feature::Decade(1990), Feature::Network("nbc".into())];
        let drama = vec![Feature::Genre("drama".into()), Feature::Decade(2010)];
        assert!(t.fit(&comedy90s) > 0.8);
        assert!(t.fit(&drama) < 0.2);
    }

    #[test]
    fn a_profile_that_watched_nothing_is_cold() {
        let t = Taste::of(&Catalog::default(), NOW);
        assert!(t.cold() && t.affinity.is_empty());
        assert_eq!(t.film_share, 0.5);
    }
}
