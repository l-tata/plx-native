//! **A recipe's programmes**: what a channel airs, read from its source and narrowed by its rules.
//!
//! Blocking; the store runs it on a worker. The reads are injected ([`Reads`]) so the rules of
//! assembly — which shows contribute, how many episodes, the caps — are graded on the host.

use super::catalog::{episode_matches, narrows, Catalog, Show};
use super::recipe::{Recipe, Source};
use super::schedule::{mix, Program, Rng};

/// Programmes a channel holds at most. A day of guide over even this many is cheap
/// (`schedule::window`); the cap bounds the reads and the memory of a channel built from a whole
/// library.
pub const PROGRAMMES_MAX: usize = 4_000;
/// Shows a rule channel reads episodes for at most; when more match, the ones read are a sample
/// chosen by the recipe's seed, so every television picks the same ones.
pub const SHOWS_MAX: usize = 48;
/// Episodes one show contributes at most, so one long-running show cannot swamp a channel.
pub const EPISODES_PER_SHOW_MAX: usize = 220;

/// The reads a build makes.
pub trait Reads {
    /// A non-library source's items (a playlist's, a collection's, a show's or season's episodes).
    fn source(&mut self, source: &Source) -> Option<Vec<Program>>;
    /// One show's episodes.
    fn episodes(&mut self, show: &Show) -> Option<Vec<Program>>;
}

/// The programmes `recipe` airs. `None` when its source could not be read at all (a channel then
/// keeps the programmes it had); an empty list when it reads but nothing matches.
pub fn programmes(recipe: &Recipe, catalog: Option<&Catalog>, reads: &mut dyn Reads) -> Option<Vec<Program>> {
    let rules = &recipe.rules;
    let mut out: Vec<Program> = match &recipe.source {
        Source::Library => {
            let cat = catalog?;
            let mut films: Vec<Program> = cat.films(rules).cloned().collect();
            let mut shows: Vec<&Show> = cat.shows_matching(rules).collect();
            if shows.len() > SHOWS_MAX {
                let mut rng = Rng::new(mix(recipe.seed, 0x7368_6f77));
                rng.shuffle(&mut shows);
                shows.truncate(SHOWS_MAX);
            }
            // Read in title order so the result does not depend on the sample's order.
            shows.sort_by(|a, b| a.title.cmp(&b.title));
            let mut eps: Vec<Program> = Vec::new();
            for s in shows {
                let Some(list) = reads.episodes(s) else { continue };
                eps.extend(list.into_iter().filter(|p| episode_matches(rules, p)).take(EPISODES_PER_SHOW_MAX));
            }
            films.sort_by(|a, b| a.title.cmp(&b.title));
            films.extend(eps);
            films
        }
        other => reads.source(other)?.into_iter().filter(|p| narrows(rules, p)).collect(),
    };
    out.retain(|p| p.dur_ms > 0 && !p.rk.is_empty());
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.rk.clone()));
    out.truncate(PROGRAMMES_MAX);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vchannel::catalog::Section;
    use crate::vchannel::recipe::Rules;

    struct Fake {
        episode_reads: usize,
    }

    impl Reads for Fake {
        fn source(&mut self, source: &Source) -> Option<Vec<Program>> {
            match source {
                Source::Show { rk } if rk == "gone" => None,
                Source::Show { rk } => Some((1..=5).map(|i| ep(rk, i, if i == 3 { 0 } else { 22 })).collect()),
                _ => Some(Vec::new()),
            }
        }
        fn episodes(&mut self, show: &Show) -> Option<Vec<Program>> {
            self.episode_reads += 1;
            Some((1..=300).map(|i| ep(&show.rk, i, 22)).collect())
        }
    }

    fn ep(show: &str, i: i64, mins: i64) -> Program {
        Program { rk: format!("{show}-{i}"), episode: true, show_rk: show.into(), index: i, dur_ms: mins * 60_000, ..Default::default() }
    }

    fn catalog(shows: usize) -> Catalog {
        Catalog {
            sid: 1,
            sections: vec![Section { key: 2, movies: false, title: "TV".into() }],
            movies: vec![Program { rk: "m".into(), title: "M".into(), dur_ms: 90 * 60_000, ..Default::default() }],
            shows: (0..shows).map(|i| Show { rk: format!("s{i:03}"), title: format!("S{i:03}"), leaf_count: 300, ..Default::default() }).collect(),
        }
    }

    #[test]
    fn a_show_channel_keeps_its_playable_episodes() {
        let r = Recipe { source: Source::Show { rk: "frasier".into() }, ..Default::default() };
        let p = programmes(&r, None, &mut Fake { episode_reads: 0 }).unwrap();
        assert_eq!(p.len(), 4, "the zero-length episode is left out");
        let r = Recipe { source: Source::Show { rk: "gone".into() }, ..Default::default() };
        assert!(programmes(&r, None, &mut Fake { episode_reads: 0 }).is_none(), "an unreadable source is None, not empty");
    }

    #[test]
    fn a_library_channel_samples_shows_and_caps_each_one() {
        let r = Recipe { source: Source::Library, seed: 5, ..Default::default() };
        let mut f = Fake { episode_reads: 0 };
        let p = programmes(&r, Some(&catalog(100)), &mut f).unwrap();
        assert_eq!(f.episode_reads, SHOWS_MAX);
        assert_eq!(p.len(), (1 + SHOWS_MAX * EPISODES_PER_SHOW_MAX).min(PROGRAMMES_MAX));
        // The same seed samples the same shows on every television.
        let again = programmes(&r, Some(&catalog(100)), &mut Fake { episode_reads: 0 }).unwrap();
        assert_eq!(p.iter().map(|x| &x.rk).collect::<Vec<_>>(), again.iter().map(|x| &x.rk).collect::<Vec<_>>());
    }

    #[test]
    fn a_library_channel_without_a_catalog_cannot_build_and_rules_apply() {
        let r = Recipe { source: Source::Library, ..Default::default() };
        assert!(programmes(&r, None, &mut Fake { episode_reads: 0 }).is_none());
        let films = Recipe { source: Source::Library, rules: Rules { kinds: crate::vchannel::recipe::Kinds::Movies, ..Default::default() }, ..Default::default() };
        let p = programmes(&films, Some(&catalog(3)), &mut Fake { episode_reads: 0 }).unwrap();
        assert_eq!(p.iter().map(|x| x.rk.as_str()).collect::<Vec<_>>(), ["m"]);
    }
}
