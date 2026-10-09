//! **Suggested channels**: the channels this profile would probably love, invented from its own
//! library and what it watches, each with a name a person would give it and a line saying why.
//!
//! The pipeline, all pure over the [`Catalog`] and the [`Taste`]:
//!
//! 1. **Ideas.** Families of candidate channels, each a [`Rules`] set the builder can read back:
//!    decades × genres ("90s Sitcoms"), genres, networks ("NBC Comedy"), studios ("Pixar"),
//!    directors, actors, collections, *because you watched…*, catch-up TV (the next episodes of
//!    what the profile is in the middle of), unwatched gems, new arrivals, moods (Saturday
//!    morning cartoons, background comedy, movie night, late-night thrills, classic cinema,
//!    documentaries) and the season (Halloween, the holidays, summer blockbusters, date night).
//!    A first pass counts what the library holds, so only ideas it could fill are built.
//! 2. **Measure.** Each idea's members come from the same rule engine a kept channel uses
//!    (`catalog::film_matches` / `show_matches`), so a suggestion airs exactly what it described.
//!    Measured: runtime, how many titles, how well they fit the profile, how much is unwatched,
//!    how much is new, and how specific it is.
//! 3. **Score.** Relevance to the profile leads; novelty, freshness and coherence follow; the time
//!    of day, the day of the week and the month lift the ideas that suit the moment (cartoons on a
//!    Saturday morning, thrillers after dark). A profile that has watched nothing yet is scored
//!    on the library's own ratings instead.
//! 4. **Choose.** Greedy maximal-marginal-relevance: each pick is the best remaining idea after a
//!    penalty for overlapping what is already picked, with a cap per family, so the row is ten
//!    different evenings rather than five comedy channels. Ideas the profile already kept are
//!    left out.
//!
//! [`surprise`] draws one coherent idea at random, weighted by score — *Surprise me*.

use std::collections::{HashMap, HashSet};

use super::catalog::{film_matches_at, now_s, show_matches, Catalog, Show};
use super::recipe::{rating_rank, Kinds, Recipe, Rules, Source};
use super::schedule::{epoch_for_random_join, mix, seed_of, Program, Rng, Style};
use super::taste::{decade_of, film_engagement, film_features, norm, show_engagement, show_features, Feature, Taste};

/// Suggestions a row offers.
pub const ROW: usize = 16;

/// The moment the suggestions are for: local time of day, day of the week, month.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    pub now_s: i64,
    /// 0..=23, local.
    pub hour: u32,
    /// 0 = Monday … 6 = Sunday, local.
    pub weekday: u32,
    /// 1..=12, local.
    pub month: u32,
}

impl Moment {
    /// The moment `now_ms` (wall clock) is, in the set's own time zone.
    pub fn at(now_ms: i64) -> Moment {
        let local_s = now_ms / 1000 + i64::from(plx_base::wallclock::local_offset_s(now_ms));
        let days = local_s.div_euclid(86_400);
        let secs = local_s.rem_euclid(86_400);
        // 1970-01-01 was a Thursday (weekday 3 counting from Monday).
        let weekday = ((days + 3).rem_euclid(7)) as u32;
        let (_, month, _) = civil_from_days(days);
        Moment { now_s: now_ms / 1000, hour: (secs / 3600) as u32, weekday, month }
    }

    fn weekend_morning(&self) -> bool {
        self.weekday >= 5 && (6..12).contains(&self.hour)
    }

    fn evening(&self) -> bool {
        (18..24).contains(&self.hour)
    }

    fn late(&self) -> bool {
        self.hour >= 21 || self.hour < 3
    }
}

/// Howard Hinnant's days-to-civil: (year, month, day) of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Where an idea came from: its family decides its naming, its prior and its cap in a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Family {
    BecauseYouWatched,
    CatchUp,
    Season,
    Mood,
    DecadeGenre,
    #[default]
    Genre,
    Network,
    Studio,
    Director,
    Actor,
    Collection,
    Gems,
    New,
}

impl Family {
    fn prior(self) -> f64 {
        match self {
            Family::BecauseYouWatched => 0.16,
            Family::CatchUp => 0.14,
            Family::Season => 0.12,
            Family::Mood => 0.07,
            Family::DecadeGenre => 0.06,
            Family::Genre => 0.02,
            Family::Network => 0.05,
            Family::Studio => 0.05,
            Family::Director => 0.05,
            Family::Actor => 0.03,
            Family::Collection => 0.06,
            Family::Gems => 0.06,
            Family::New => 0.05,
        }
    }

    /// At most this many of a family in one row.
    fn cap(self) -> usize {
        match self {
            Family::BecauseYouWatched => 3,
            Family::DecadeGenre | Family::Mood => 3,
            Family::CatchUp | Family::Season | Family::Gems | Family::New | Family::Genre => 1,
            _ => 2,
        }
    }

    fn min_hours(self) -> f64 {
        match self {
            Family::Director | Family::Season | Family::Collection | Family::New => 4.0,
            Family::CatchUp | Family::BecauseYouWatched | Family::Studio | Family::Actor => 6.0,
            _ => 8.0,
        }
    }

    fn min_titles(self) -> usize {
        match self {
            Family::CatchUp => 2,
            Family::Director | Family::Collection | Family::Mood | Family::Season => 3,
            _ => 4,
        }
    }
}

/// One suggested channel.
#[derive(Clone, Debug, Default)]
pub struct Suggestion {
    /// Stable for the same idea over the same library, so a kept one is not suggested again.
    pub id: String,
    pub family: Family,
    pub name: String,
    /// "6 shows · 312 episodes · 114 hours".
    pub tagline: String,
    /// Why the profile is shown it: "Because you watch a lot of comedy".
    pub why: String,
    pub rules: Rules,
    pub style: Style,
    pub score: f64,
    pub films: usize,
    pub shows: usize,
    pub programmes: usize,
    pub hours: f64,
    /// The backdrop the tile shows (server, path), and up to four posters for its collage.
    pub art: Option<(u16, String)>,
    pub posters: Vec<(u16, String)>,
    /// What is on first, for the tile: a title from the channel the profile would recognise.
    pub sample: Vec<String>,
}

impl Suggestion {
    /// The recipe Keep makes of it: library-sourced, a seed of its own, joined mid-programme.
    pub fn recipe(&self, now_ms: i64) -> Recipe {
        let seed = mix(seed_of(&self.id), now_ms as u64);
        let est_pass_ms = (self.hours * 3_600_000.0) as i64;
        Recipe {
            name: self.name.clone(),
            source: Source::Library,
            rules: self.rules.clone(),
            style: self.style,
            seed,
            epoch_ms: epoch_for_random_join(est_pass_ms.max(1), now_ms, seed),
            origin: self.id.clone(),
            tagline: self.why.clone(),
            ..Default::default()
        }
    }
}

/// An idea before it is measured.
#[derive(Clone, Debug)]
struct Idea {
    id: String,
    family: Family,
    name: String,
    why: String,
    rules: Rules,
    style: Style,
    boost: f64,
}

/// What an idea's members add up to.
#[derive(Clone, Debug, Default)]
struct Measure {
    films: usize,
    shows: usize,
    programmes: usize,
    ms: f64,
    fit: f64,
    rating: f64,
    unwatched: f64,
    fresh: f64,
    keys: HashSet<String>,
    best: Vec<(f64, u16, String, String, String)>, // (rank, sid, art, thumb, title)
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// "90s", "2000s", "1950s".
pub fn decade_label(d: i64) -> String {
    match d {
        1960 | 1970 | 1980 | 1990 => format!("{}s", d % 100),
        _ => format!("{d}s"),
    }
}

/// What a channel of `genre` is called: films and series take different nouns, and a comedy of
/// short episodes is a sitcom.
pub fn genre_noun(genre: &str, kinds: Kinds, short: bool) -> String {
    let g = norm(genre);
    match kinds {
        Kinds::Episodes => match g.as_str() {
            "comedy" if short => "Sitcoms".into(),
            "comedy" => "Comedy Series".into(),
            "drama" => "Dramas".into(),
            "crime" => "Crime Dramas".into(),
            "animation" => "Cartoons".into(),
            "documentary" => "Docuseries".into(),
            "science fiction" | "sci-fi" | "sci-fi & fantasy" => "Sci-Fi Series".into(),
            "reality" => "Reality TV".into(),
            "mystery" => "Mysteries".into(),
            "thriller" | "suspense" => "Thrillers".into(),
            "children" | "kids" | "family" => "Kids' Shows".into(),
            "game show" => "Game Shows".into(),
            "talk show" | "talk" => "Talk Shows".into(),
            "western" => "Westerns".into(),
            "anime" => "Anime".into(),
            _ => format!("{} Shows", title_case(&g)),
        },
        Kinds::Movies => match g.as_str() {
            "comedy" => "Comedies".into(),
            "drama" => "Dramas".into(),
            "documentary" => "Documentaries".into(),
            "thriller" | "suspense" => "Thrillers".into(),
            "romance" => "Romances".into(),
            "mystery" => "Mysteries".into(),
            "western" => "Westerns".into(),
            "musical" | "music" => "Musicals".into(),
            "science fiction" | "sci-fi" => "Sci-Fi Movies".into(),
            "animation" => "Animated Movies".into(),
            "family" => "Family Movies".into(),
            _ => format!("{} Movies", title_case(&g)),
        },
        Kinds::Both => title_case(&g),
    }
}

fn kinds_word(k: Kinds) -> &'static str {
    match k {
        Kinds::Movies => "films",
        Kinds::Episodes => "shows",
        Kinds::Both => "titles",
    }
}

/// A credit that names nobody in particular, which no channel should be built around.
fn placeholder(name: &str) -> bool {
    matches!(norm(name).as_str(), "" | "various" | "various artists" | "unknown" | "n/a" | "none" | "anonymous")
}

/// Counts over the catalog, so only ideas the library can fill are built.
#[derive(Default)]
struct Census {
    genre_decade: HashMap<(String, i64, bool), usize>, // (genre, decade, is_show) → titles
    genre: HashMap<(String, bool), usize>,
    network: HashMap<String, Vec<usize>>,  // show indices
    studio: HashMap<String, usize>,        // films
    director: HashMap<String, usize>,
    actor: HashMap<String, usize>,
    collection: HashMap<String, usize>,
    display: HashMap<String, String>, // normalised → as the library spells it
}

impl Census {
    fn of(cat: &Catalog) -> Census {
        let mut c = Census::default();
        let name = |c: &mut Census, s: &str| {
            let n = norm(s);
            c.display.entry(n.clone()).or_insert_with(|| s.trim().to_owned());
            n
        };
        for p in &cat.movies {
            for g in &p.genres {
                let g = name(&mut c, g);
                *c.genre.entry((g.clone(), false)).or_default() += 1;
                if let Some(d) = decade_of(p.year) {
                    *c.genre_decade.entry((g, d, false)).or_default() += 1;
                }
            }
            if !p.studio.is_empty() {
                let s = name(&mut c, &p.studio);
                *c.studio.entry(s).or_default() += 1;
            }
            for d in &p.directors {
                let d = name(&mut c, d);
                *c.director.entry(d).or_default() += 1;
            }
            for a in p.actors.iter().take(3) {
                let a = name(&mut c, a);
                *c.actor.entry(a).or_default() += 1;
            }
            for col in &p.collections {
                let col = name(&mut c, col);
                *c.collection.entry(col).or_default() += 1;
            }
        }
        for (i, s) in cat.shows.iter().enumerate() {
            for g in &s.genres {
                let g = name(&mut c, g);
                *c.genre.entry((g.clone(), true)).or_default() += 1;
                if let Some(d) = decade_of(s.year) {
                    *c.genre_decade.entry((g, d, true)).or_default() += 1;
                }
            }
            if !s.studio.is_empty() {
                let n = name(&mut c, &s.studio);
                c.network.entry(n).or_default().push(i);
            }
            for a in s.actors.iter().take(3) {
                let a = name(&mut c, a);
                *c.actor.entry(a).or_default() += 1;
            }
            for col in &s.collections {
                let col = name(&mut c, col);
                *c.collection.entry(col).or_default() += 1;
            }
        }
        c
    }

    fn shown(&self, n: &str) -> String {
        self.display.get(n).cloned().unwrap_or_else(|| title_case(n))
    }
}

fn kinds_of(show: bool) -> Kinds {
    if show { Kinds::Episodes } else { Kinds::Movies }
}

fn style_for(kinds: Kinds, genre: &str, short: bool) -> Style {
    match (kinds, norm(genre).as_str()) {
        (Kinds::Movies, _) => Style::Random,
        (Kinds::Episodes, _) if short => Style::RoundRobin,
        (Kinds::Episodes, "drama" | "crime" | "mystery" | "thriller" | "science fiction" | "sci-fi") => Style::Blocks,
        (Kinds::Episodes, _) => Style::RoundRobin,
        _ => Style::Random,
    }
}

/// Every idea the library could fill, from every family.
fn ideas(cat: &Catalog, taste: &Taste, m: &Moment) -> Vec<Idea> {
    let c = Census::of(cat);
    let mut out: Vec<Idea> = Vec::new();
    let short_comedy = |genre: &str| -> bool {
        // A genre's shows are "short" when their typical episode runs under 35 minutes.
        let g = norm(genre);
        let (sum, n) = cat
            .shows
            .iter()
            .filter(|s| s.genres.iter().any(|x| norm(x) == g) && s.episode_ms > 0)
            .fold((0i64, 0i64), |(a, n), s| (a + s.episode_ms, n + 1));
        n > 0 && sum / n <= 35 * 60_000
    };

    // Decades × genres.
    for ((g, d, show), n) in &c.genre_decade {
        if *n < if *show { 3 } else { 6 } {
            continue;
        }
        let kinds = kinds_of(*show);
        let short = *show && short_comedy(g);
        let gl = c.shown(g);
        let name = format!("{} {}", decade_label(*d), genre_noun(&gl, kinds, short));
        let why = if taste.of_feature(&Feature::Genre(g.clone())) >= 0.5 {
            format!("Because you watch a lot of {}", g)
        } else if taste.of_feature(&Feature::Decade(*d)) >= 0.5 {
            format!("You have a soft spot for the {}", decade_label(*d))
        } else {
            format!("{} {} from the {}", n, kinds_word(kinds), decade_label(*d))
        };
        out.push(Idea {
            id: format!("decade:{d}:{g}:{}", kinds_word(kinds)),
            family: Family::DecadeGenre,
            name,
            why,
            rules: Rules { kinds, genres: vec![gl.clone()], year_from: *d, year_to: d + 9, ..Default::default() },
            style: style_for(kinds, g, short),
            boost: 0.0,
        });
    }
    // Genres alone, for the profile's strongest genres.
    for (g, a) in taste.top_genres(4) {
        for show in [true, false] {
            if c.genre.get(&(g.clone(), show)).copied().unwrap_or(0) < if show { 4 } else { 10 } {
                continue;
            }
            let kinds = kinds_of(show);
            let short = show && short_comedy(&g);
            let gl = c.shown(&g);
            out.push(Idea {
                id: format!("genre:{g}:{}", kinds_word(kinds)),
                family: Family::Genre,
                name: format!("All {}", genre_noun(&gl, kinds, short)),
                why: format!("Your most-watched genre{}", if a >= 0.99 { "" } else { "s include this" }),
                rules: Rules { kinds, genres: vec![gl], ..Default::default() },
                style: style_for(kinds, &g, short),
                boost: 0.0,
            });
        }
    }
    // Networks: all of one network's shows, and its dominant genre's.
    for (n, idx) in &c.network {
        if idx.len() < 3 {
            continue;
        }
        let shown = c.shown(n);
        let watched = idx
            .iter()
            .map(|&i| &cat.shows[i])
            .filter(|s| s.viewed_leaf_count > 0)
            .max_by(|a, b| show_engagement(a, m.now_s).partial_cmp(&show_engagement(b, m.now_s)).unwrap_or(std::cmp::Ordering::Equal));
        let why = match watched {
            Some(s) => format!("Because you watch {}", s.title),
            None => format!("Every {shown} show in your library"),
        };
        out.push(Idea {
            id: format!("network:{n}"),
            family: Family::Network,
            name: format!("{shown} Shows"),
            why: why.clone(),
            rules: Rules { kinds: Kinds::Episodes, studios: vec![shown.clone()], ..Default::default() },
            style: Style::RoundRobin,
            boost: 0.0,
        });
        let mut genres: HashMap<String, usize> = HashMap::new();
        for &i in idx {
            for g in &cat.shows[i].genres {
                *genres.entry(norm(g)).or_default() += 1;
            }
        }
        if let Some((g, k)) = genres.into_iter().max_by_key(|(g, k)| (*k, std::cmp::Reverse(g.clone()))) {
            if k >= 3 && k * 10 >= idx.len() * 6 {
                let short = short_comedy(&g);
                let gl = c.shown(&g);
                out.push(Idea {
                    id: format!("network:{n}:{g}"),
                    family: Family::Network,
                    name: format!("{shown} {}", genre_noun(&gl, Kinds::Episodes, short).replace("Sitcoms", "Comedy")),
                    why,
                    rules: Rules { kinds: Kinds::Episodes, studios: vec![shown], genres: vec![gl], ..Default::default() },
                    style: style_for(Kinds::Episodes, &g, short),
                    boost: 0.0,
                });
            }
        }
    }
    // Studios' films.
    for (s, n) in &c.studio {
        if *n < 5 || placeholder(s) {
            continue;
        }
        let shown = c.shown(s);
        let bare = ["pixar", "studio ghibli", "dreamworks animation", "walt disney animation studios", "a24", "marvel studios", "lucasfilm"];
        let name = if bare.iter().any(|b| s == b) { shown.clone() } else { format!("{shown} Films") };
        out.push(Idea {
            id: format!("studio:{s}"),
            family: Family::Studio,
            name,
            why: format!("{n} films from {shown}"),
            rules: Rules { kinds: Kinds::Movies, studios: vec![shown], ..Default::default() },
            style: Style::Random,
            boost: 0.0,
        });
    }
    // Directors and actors.
    let seen_by = |person: &str, films_only: bool| -> usize {
        let p = norm(person);
        let films = cat.movies.iter().filter(|f| f.watched && (f.directors.iter().chain(f.actors.iter().take(3))).any(|x| norm(x) == p)).count();
        let shows = if films_only { 0 } else { cat.shows.iter().filter(|s| s.viewed_leaf_count > 0 && s.actors.iter().take(3).any(|x| norm(x) == p)).count() };
        films + shows
    };
    for (d, n) in &c.director {
        if *n < 4 || placeholder(d) {
            continue;
        }
        let shown = c.shown(d);
        let k = seen_by(d, true);
        out.push(Idea {
            id: format!("director:{d}"),
            family: Family::Director,
            name: format!("{shown} Films"),
            why: if k > 0 { format!("You've seen {k} of their {n} films") } else { format!("All {n} of {shown}'s films you have") },
            rules: Rules { kinds: Kinds::Movies, directors: vec![shown], ..Default::default() },
            style: Style::Random,
            boost: 0.0,
        });
    }
    for (a, n) in &c.actor {
        if *n < 5 || placeholder(a) {
            continue;
        }
        let shown = c.shown(a);
        let k = seen_by(a, false);
        out.push(Idea {
            id: format!("actor:{a}"),
            family: Family::Actor,
            name: format!("Starring {shown}"),
            why: if k > 0 { format!("You keep coming back to {shown}") } else { format!("{n} titles starring {shown}") },
            rules: Rules { actors: vec![shown], ..Default::default() },
            style: Style::Random,
            boost: 0.0,
        });
    }
    // Collections.
    for (col, n) in &c.collection {
        if *n < 3 {
            continue;
        }
        let shown = c.shown(col);
        out.push(Idea {
            id: format!("collection:{col}"),
            family: Family::Collection,
            name: shown.clone(),
            why: format!("From your {} collection", shown.trim_end_matches(" Collection")),
            rules: Rules { collections: vec![shown], ..Default::default() },
            style: Style::Random,
            boost: 0.0,
        });
    }
    // Because you watched: the most engaged recent titles as seeds, their nearest neighbours as the
    // channel.
    out.extend(because_you_watched(cat, taste, m));
    // Catch-up: the next episodes of shows the profile is in the middle of.
    let started: Vec<&Show> = cat
        .shows
        .iter()
        .filter(|s| s.viewed_leaf_count > 0 && s.unwatched() > 0 && m.now_s - s.last_viewed_at <= 240 * 86_400)
        .collect();
    if started.len() >= 2 {
        out.push(Idea {
            id: "catchup".into(),
            family: Family::CatchUp,
            name: "Catch-Up TV".into(),
            why: format!("The next episodes of {} shows you're in the middle of", started.len()),
            rules: Rules { kinds: Kinds::Episodes, shows: started.iter().map(|s| s.rk.clone()).collect(), unwatched_only: true, ..Default::default() },
            style: Style::RoundRobin,
            boost: 0.05,
        });
    }
    // Unwatched gems, new arrivals.
    out.push(Idea {
        id: "gems".into(),
        family: Family::Gems,
        name: "Unwatched Gems".into(),
        why: "Highly rated, and you haven't seen them yet".into(),
        rules: Rules { kinds: Kinds::Movies, min_score: 7.5, unwatched_only: true, ..Default::default() },
        style: Style::Random,
        boost: 0.0,
    });
    out.push(Idea {
        id: "new".into(),
        family: Family::New,
        name: "New This Month".into(),
        why: "Everything added to your library in the last 30 days".into(),
        rules: Rules { added_within_days: 30, ..Default::default() },
        style: Style::Random,
        boost: 0.03,
    });
    out.extend(moods(m));
    out.extend(season(m));
    out
}

fn moods(m: &Moment) -> Vec<Idea> {
    let idea = |id: &str, name: &str, why: &str, rules: Rules, style: Style, boost: f64| Idea {
        id: id.into(),
        family: Family::Mood,
        name: name.into(),
        why: why.into(),
        rules,
        style,
        boost,
    };
    vec![
        idea(
            "mood:saturday",
            "Saturday Morning Cartoons",
            if m.weekend_morning() { "It's the weekend — cartoons, rated PG and under" } else { "Cartoons and kids' shows, rated PG and under" },
            Rules { kinds: Kinds::Episodes, genres: vec!["Animation".into(), "Children".into(), "Kids".into()], max_rating_rank: 2, max_minutes: 35, ..Default::default() },
            Style::RoundRobin,
            if m.weekend_morning() { 0.35 } else { 0.0 },
        ),
        idea(
            "mood:background",
            "Background Comedy",
            "Short comedies to have on",
            Rules { kinds: Kinds::Episodes, genres: vec!["Comedy".into()], max_minutes: 32, ..Default::default() },
            Style::RoundRobin,
            if (17..23).contains(&m.hour) { 0.04 } else { 0.0 },
        ),
        idea(
            "mood:movienight",
            "Movie Night",
            "Feature films with strong reviews",
            Rules { kinds: Kinds::Movies, min_minutes: 85, max_minutes: 150, min_score: 7.0, ..Default::default() },
            Style::Random,
            if m.evening() { 0.18 } else { 0.0 },
        ),
        idea(
            "mood:latenight",
            "Late Night Thrills",
            if m.late() { "For after dark" } else { "Horror and thrillers, for after dark" },
            Rules { genres: vec!["Horror".into(), "Thriller".into(), "Suspense".into()], ..Default::default() },
            Style::Random,
            if m.late() { 0.22 } else { 0.0 },
        ),
        idea(
            "mood:classics",
            "Classic Cinema",
            "Films from before 1975",
            Rules { kinds: Kinds::Movies, year_to: 1974, ..Default::default() },
            Style::Random,
            0.0,
        ),
        idea(
            "mood:docs",
            "Documentary Hour",
            "Documentaries and docuseries",
            Rules { genres: vec!["Documentary".into()], ..Default::default() },
            Style::Random,
            0.0,
        ),
    ]
}

fn season(m: &Moment) -> Vec<Idea> {
    let idea = |id: &str, name: &str, why: &str, rules: Rules| Idea {
        id: id.into(),
        family: Family::Season,
        name: name.into(),
        why: why.into(),
        rules,
        style: Style::Random,
        boost: 0.3,
    };
    match m.month {
        10 => vec![idea("season:halloween", "Halloween Horror", "It's October", Rules {
            genres: vec!["Horror".into()],
            ..Default::default()
        })],
        12 => vec![idea("season:holiday", "Holiday Classics", "'Tis the season", Rules {
            keywords: vec!["christmas".into(), "holiday".into(), "santa".into(), "xmas".into(), "noel".into()],
            ..Default::default()
        })],
        2 => vec![idea("season:datenight", "Date Night", "Romance for February", Rules {
            kinds: Kinds::Movies,
            genres: vec!["Romance".into()],
            ..Default::default()
        })],
        6..=8 => vec![idea("season:summer", "Summer Blockbusters", "Big summer movies", Rules {
            kinds: Kinds::Movies,
            genres: vec!["Action".into(), "Adventure".into()],
            min_score: 6.0,
            ..Default::default()
        })],
        _ => Vec::new(),
    }
}

/// How alike two titles are, 0..=1: shared genres lead, then network or studio, era and people.
fn similarity(a: &[Feature], b: &[Feature]) -> f64 {
    let genres = |v: &[Feature]| v.iter().filter(|f| matches!(f, Feature::Genre(_))).cloned().collect::<HashSet<_>>();
    let (ga, gb) = (genres(a), genres(b));
    let jac = if ga.is_empty() || gb.is_empty() { 0.0 } else { ga.intersection(&gb).count() as f64 / ga.union(&gb).count() as f64 };
    let net = |v: &[Feature]| v.iter().find_map(|f| if let Feature::Network(n) = f { Some(n.clone()) } else { None });
    let same_net = matches!((net(a), net(b)), (Some(x), Some(y)) if x == y);
    let dec = |v: &[Feature]| v.iter().find_map(|f| if let Feature::Decade(d) = f { Some(*d) } else { None });
    let era = match (dec(a), dec(b)) {
        (Some(x), Some(y)) => (1.0 - ((x - y).abs() as f64) / 30.0).max(0.0),
        _ => 0.3,
    };
    let people = |v: &[Feature]| v.iter().filter(|f| matches!(f, Feature::Person(_))).cloned().collect::<HashSet<_>>();
    let shared_people = !people(a).is_disjoint(&people(b));
    0.5 * jac + 0.2 * f64::from(u8::from(same_net)) + 0.18 * era + 0.12 * f64::from(u8::from(shared_people))
}

fn because_you_watched(cat: &Catalog, taste: &Taste, m: &Moment) -> Vec<Idea> {
    // Seeds: the three most engaged shows and two most engaged films, recency included.
    let mut shows: Vec<(&Show, f64)> = cat.shows.iter().map(|s| (s, show_engagement(s, m.now_s))).filter(|(_, e)| *e > 0.15).collect();
    shows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut films: Vec<(&Program, f64)> = cat.movies.iter().map(|p| (p, film_engagement(p, m.now_s))).filter(|(_, e)| *e > 0.5).collect();
    films.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    let _ = taste;
    for (seed, _) in shows.into_iter().take(3) {
        let sf = show_features(seed);
        let mut near: Vec<(&Show, f64)> = cat
            .shows
            .iter()
            .filter(|s| s.rk != seed.rk)
            .map(|s| (s, similarity(&sf, &show_features(s))))
            .filter(|(_, sim)| *sim >= 0.42)
            .collect();
        near.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.title.cmp(&b.0.title)));
        near.truncate(10);
        if near.len() < 2 {
            continue;
        }
        let mut keys: Vec<String> = vec![seed.rk.clone()];
        keys.extend(near.iter().map(|(s, _)| s.rk.clone()));
        out.push(Idea {
            id: format!("because:{}", seed.rk),
            family: Family::BecauseYouWatched,
            name: format!("Because You Watched {}", seed.title),
            why: format!("{} and {} shows like it", seed.title, near.len()),
            rules: Rules { kinds: Kinds::Episodes, shows: keys, ..Default::default() },
            style: Style::Random,
            boost: 0.0,
        });
    }
    for (seed, _) in films.into_iter().take(2) {
        let sf = film_features(seed);
        let mut near: Vec<(&Program, f64)> = cat
            .movies
            .iter()
            .filter(|p| p.rk != seed.rk)
            .map(|p| (p, similarity(&sf, &film_features(p))))
            .filter(|(_, sim)| *sim >= 0.45)
            .collect();
        near.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.title.cmp(&b.0.title)));
        near.truncate(24);
        if near.len() < 4 {
            continue;
        }
        let mut keys: Vec<String> = vec![seed.rk.clone()];
        keys.extend(near.iter().map(|(p, _)| p.rk.clone()));
        out.push(Idea {
            id: format!("because:{}", seed.rk),
            family: Family::BecauseYouWatched,
            name: format!("Because You Watched {}", seed.title),
            why: format!("Films like {}", seed.title),
            rules: Rules { kinds: Kinds::Movies, films: keys, ..Default::default() },
            style: Style::Random,
            boost: 0.0,
        });
    }
    out
}

fn measure(cat: &Catalog, taste: &Taste, idea: &Idea, now_s: i64) -> Measure {
    let r = &idea.rules;
    let mut ms = Measure::default();
    let (mut fit_w, mut rating_w, mut unwatched_w, mut fresh_n) = (0.0, 0.0, 0.0, 0usize);
    let fresh_since = now_s - 60 * 86_400;
    for p in cat.movies.iter().filter(|p| film_matches_at(r, p, now_s)) {
        let dur = p.dur_ms.max(1) as f64;
        ms.films += 1;
        ms.ms += dur;
        let fit = taste.fit(&film_features(p));
        fit_w += fit * dur;
        rating_w += p.audience_rating / 10.0 * dur;
        if !p.watched {
            unwatched_w += dur;
        }
        if p.added_at >= fresh_since {
            fresh_n += 1;
        }
        ms.keys.insert(p.rk.clone());
        let rank = film_engagement(p, now_s) * 2.0 + p.audience_rating / 10.0 + fit;
        ms.best.push((rank, p.sid, p.art.clone(), p.thumb.clone(), p.title.clone()));
    }
    for s in cat.shows.iter().filter(|s| show_matches(r, s)) {
        let eps = if r.unwatched_only { s.unwatched() } else { s.leaf_count.max(0) };
        if eps == 0 {
            continue;
        }
        let dur = (eps * s.episode_ms.max(1)) as f64;
        ms.shows += 1;
        ms.programmes += eps as usize;
        ms.ms += dur;
        let fit = taste.fit(&show_features(s));
        fit_w += fit * dur;
        rating_w += s.audience_rating / 10.0 * dur;
        unwatched_w += dur * (s.unwatched() as f64 / s.leaf_count.max(1) as f64);
        if s.added_at >= fresh_since || s.last_added_at >= fresh_since {
            fresh_n += 1;
        }
        ms.keys.insert(s.rk.clone());
        let rank = show_engagement(s, now_s) * 2.0 + s.audience_rating / 10.0 + fit;
        ms.best.push((rank, s.sid, s.art.clone(), s.thumb.clone(), s.title.clone()));
    }
    ms.programmes += ms.films;
    if ms.ms > 0.0 {
        ms.fit = fit_w / ms.ms;
        ms.rating = rating_w / ms.ms;
        ms.unwatched = unwatched_w / ms.ms;
    }
    let titles = (ms.films + ms.shows).max(1);
    ms.fresh = fresh_n as f64 / titles as f64;
    ms.best.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.4.cmp(&b.4)));
    ms
}

fn tagline(m: &Measure) -> String {
    let mut parts = Vec::new();
    if m.shows > 0 {
        parts.push(format!("{} show{}", m.shows, if m.shows == 1 { "" } else { "s" }));
        let eps = m.programmes - m.films;
        parts.push(format!("{eps} episodes"));
    }
    if m.films > 0 {
        parts.push(format!("{} film{}", m.films, if m.films == 1 { "" } else { "s" }));
    }
    let hours = m.ms / 3_600_000.0;
    parts.push(if hours >= 48.0 { format!("{:.0} days", hours / 24.0) } else { format!("{hours:.0} hours") });
    parts.join(" · ")
}

/// Score one measured idea.
fn score(taste: &Taste, idea: &Idea, ms: &Measure, library_programmes: usize) -> f64 {
    let relevance = if taste.cold() { ms.rating } else { 0.8 * ms.fit + 0.2 * ms.rating };
    let coherence = 1.0 - ((ms.programmes as f64) / (library_programmes.max(1) as f64)).sqrt();
    0.42 * relevance + 0.14 * ms.unwatched + 0.08 * ms.fresh + 0.14 * coherence + idea.family.prior() + idea.boost
}

/// Jaccard overlap of two member sets.
fn overlap(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

struct Scored {
    s: Suggestion,
    keys: HashSet<String>,
}

fn scored(cat: &Catalog, taste: &Taste, m: &Moment) -> Vec<Scored> {
    let library_programmes = cat.movies.len() + cat.shows.iter().map(|s| s.leaf_count.max(0) as usize).sum::<usize>();
    let mut out: Vec<Scored> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for idea in ideas(cat, taste, m) {
        let ms = measure(cat, taste, &idea, m.now_s);
        let hours = ms.ms / 3_600_000.0;
        if hours < idea.family.min_hours() || ms.films + ms.shows < idea.family.min_titles() {
            continue;
        }
        if !seen_names.insert(idea.name.to_lowercase()) {
            continue;
        }
        // A little of the day in every score, so the row turns over from one day to the next
        // while staying put within a day (and identical on every television that day).
        let day = (m.now_s.div_euclid(86_400)) as u64;
        let jitter = (mix(seed_of(&idea.id), day) % 1000) as f64 / 1000.0 * 0.04;
        let sc = score(taste, &idea, &ms, library_programmes) + jitter;
        let art = ms.best.iter().find(|b| !b.2.is_empty()).map(|b| (b.1, b.2.clone()));
        let posters = ms.best.iter().filter(|b| !b.3.is_empty()).take(4).map(|b| (b.1, b.3.clone())).collect();
        let sample = ms.best.iter().take(3).map(|b| b.4.clone()).collect();
        out.push(Scored {
            s: Suggestion {
                id: idea.id,
                family: idea.family,
                name: idea.name,
                tagline: tagline(&ms),
                why: idea.why,
                rules: idea.rules,
                style: idea.style,
                score: sc,
                films: ms.films,
                shows: ms.shows,
                programmes: ms.programmes,
                hours,
                art,
                posters,
                sample,
            },
            keys: ms.keys,
        });
    }
    out.sort_by(|a, b| b.s.score.partial_cmp(&a.s.score).unwrap_or(std::cmp::Ordering::Equal).then(a.s.id.cmp(&b.s.id)));
    out
}

/// **The row**: up to [`ROW`] suggestions for this profile at this moment, best first, diverse,
/// and none the profile already kept (`kept`: the origins of its channels).
pub fn suggest(cat: &Catalog, m: &Moment, kept: &[String]) -> Vec<Suggestion> {
    suggest_excluding(cat, m, kept, &[])
}

/// [`suggest`], also leaving out ideas the profile said it is not interested in.
pub fn suggest_excluding(cat: &Catalog, m: &Moment, kept: &[String], dismissed: &[String]) -> Vec<Suggestion> {
    let taste = Taste::of(cat, m.now_s);
    let mut pool: Vec<Scored> = scored(cat, &taste, m)
        .into_iter()
        .filter(|x| !kept.contains(&x.s.id) && !dismissed.contains(&x.s.id))
        .collect();
    let mut chosen: Vec<Scored> = Vec::new();
    let mut per_family: HashMap<Family, usize> = HashMap::new();
    const LAMBDA: f64 = 0.55;
    while chosen.len() < ROW && !pool.is_empty() {
        let mut best: Option<(usize, f64)> = None;
        for (i, cand) in pool.iter().enumerate() {
            if per_family.get(&cand.s.family).copied().unwrap_or(0) >= cand.s.family.cap() {
                continue;
            }
            let redundancy = chosen.iter().map(|c| overlap(&c.keys, &cand.keys)).fold(0.0, f64::max);
            if redundancy > 0.85 {
                continue;
            }
            let v = cand.s.score - LAMBDA * redundancy;
            if best.map_or(true, |(_, b)| v > b) {
                best = Some((i, v));
            }
        }
        let Some((i, _)) = best else { break };
        let pick = pool.swap_remove(i);
        *per_family.entry(pick.s.family).or_default() += 1;
        chosen.push(pick);
    }
    chosen.into_iter().map(|c| c.s).collect()
}

/// **Surprise me**: one coherent idea, drawn at random weighted by score (squared, so good ideas
/// are likelier but not certain), never one already kept or in `exclude`.
pub fn surprise(cat: &Catalog, m: &Moment, kept: &[String], exclude: &[String], seed: u64) -> Option<Suggestion> {
    let taste = Taste::of(cat, m.now_s);
    let pool: Vec<Scored> = scored(cat, &taste, m)
        .into_iter()
        .filter(|x| !kept.contains(&x.s.id) && !exclude.contains(&x.s.id))
        .take(60)
        .collect();
    let weights: Vec<f64> = pool.iter().map(|x| x.s.score.max(0.01).powi(2)).collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut rng = Rng::new(seed);
    let mut t = (rng.next_u64() as f64 / u64::MAX as f64) * total;
    for (x, w) in pool.into_iter().zip(weights) {
        if t <= w {
            return Some(x.s);
        }
        t -= w;
    }
    None
}

/// Ratings that admit a kids' channel, for tests and the builder's presets.
pub fn kids_rank() -> u8 {
    rating_rank("PG")
}

#[allow(dead_code)]
fn _now() -> i64 {
    now_s()
}

#[cfg(test)]
#[path = "suggest_tests.rs"]
mod tests;
