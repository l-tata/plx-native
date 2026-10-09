//! **A virtual channel's timeline**, computed rather than stored.
//!
//! A channel is a recipe: its programmes, an order [`Style`], a seed and the moment it started
//! ([`Schedule::epoch_ms`]). The timeline is an endless run of PASSES; each pass airs every
//! programme once, in an order derived from the seed and the pass number, so every pass is a fresh
//! order while staying the same on every television that holds the same recipe. What is on at any
//! instant is arithmetic: the time since the epoch, divided by the length of a pass, names the pass
//! and the place in it ([`Schedule::at`]). Nothing about the timeline is ever written down, which is
//! what lets a channel show a week of guide for free and lets two televisions agree to the second.
//!
//! Pure: no clock, no I/O. Callers pass the instant they mean.

use std::sync::Arc;

/// One programme a channel can air: the facts the guide, the banner and the player need, and the
/// facts the order styles group by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    /// The server's ratingKey; with [`Program::sid`], the item playback resolves.
    pub rk: String,
    /// The server the programme lives on (`ServerId::raw`).
    pub sid: u16,
    pub episode: bool,
    /// The film's title, or the episode's own title.
    pub title: String,
    /// An episode's show, empty for a film.
    pub show_title: String,
    /// An episode's show ratingKey (the key the styles group by); empty for a film.
    pub show_rk: String,
    pub season: i64,
    pub index: i64,
    pub dur_ms: i64,
    pub year: i64,
    pub genres: Vec<String>,
    pub summary: String,
    pub content_rating: String,
    pub studio: String,
    /// Poster/still and backdrop paths on the item's server.
    pub thumb: String,
    pub art: String,
    pub audience_rating: f64,
    pub watched: bool,
    /// The library section the programme is in.
    pub section: i64,
    pub directors: Vec<String>,
    pub actors: Vec<String>,
    pub collections: Vec<String>,
    pub labels: Vec<String>,
    /// Unix seconds; 0 when the server did not say.
    pub added_at: i64,
    pub last_viewed_at: i64,
}

impl Program {
    /// The group the styles keep together: an episode's show, or every film on its own.
    fn group(&self) -> &str {
        if self.episode && !self.show_rk.is_empty() { &self.show_rk } else { &self.rk }
    }

    /// The key the spreading pass keeps apart: an episode's show; a film is its own.
    fn spread_key(&self) -> &str {
        self.group()
    }

    /// The order a show's episodes air in: season, then episode, then title.
    fn natural_key(&self) -> (i64, i64, &str) {
        (self.season, self.index, &self.title)
    }

    /// `S2 · E5` for an episode, empty for a film.
    pub fn episode_code(&self) -> String {
        if self.episode && (self.season > 0 || self.index > 0) {
            format!("S{} · E{}", self.season.max(0), self.index.max(0))
        } else {
            String::new()
        }
    }

    /// The guide line: the show for an episode, the title for a film.
    pub fn headline(&self) -> &str {
        if self.episode && !self.show_title.is_empty() { &self.show_title } else { &self.title }
    }
}

/// How a pass is ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Style {
    /// Every programme in a fresh random order each pass, with the same show kept off consecutive
    /// slots wherever the mix allows.
    #[default]
    Random,
    /// One episode of each show in turn, each show's episodes in order: a network's rotation.
    RoundRobin,
    /// Two or three consecutive episodes of a show, the blocks in a random order: a marathon-ish
    /// rhythm.
    Blocks,
    /// The source's own order, every pass the same.
    InOrder,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Random, Style::RoundRobin, Style::Blocks, Style::InOrder];

    /// The spelling the recipe stores.
    pub fn key(self) -> &'static str {
        match self {
            Style::Random => "random",
            Style::RoundRobin => "rotation",
            Style::Blocks => "blocks",
            Style::InOrder => "order",
        }
    }

    pub fn from_key(key: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// One programme placed on the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// Index into [`Schedule::programs`].
    pub program: usize,
    pub start_ms: i64,
    pub stop_ms: i64,
    /// Which pass this slot belongs to, and its place in that pass.
    pub pass: i64,
    pub pos: usize,
}

impl Slot {
    pub fn covers(&self, t: i64) -> bool {
        self.start_ms <= t && t < self.stop_ms
    }
}

/// A channel's timeline: its programmes (only those with a length), how they are ordered, the seed
/// and the start.
#[derive(Clone, Debug)]
pub struct Schedule {
    programs: Arc<Vec<Program>>,
    style: Style,
    seed: u64,
    epoch_ms: i64,
    pass_ms: i64,
    /// The last few pass orders worked out, shared by clones (a guide window and the player ask
    /// about the same one or two passes over and over).
    cache: Arc<std::sync::Mutex<Vec<(i64, Arc<Vec<usize>>)>>>,
}

/// Pass orders a schedule keeps.
const CACHED_PASSES: usize = 3;

impl Schedule {
    /// A schedule over `programs`, dropping any without a length (it cannot be placed).
    pub fn new(programs: Vec<Program>, style: Style, seed: u64, epoch_ms: i64) -> Schedule {
        let programs: Vec<Program> = programs.into_iter().filter(|p| p.dur_ms > 0).collect();
        let pass_ms = programs.iter().map(|p| p.dur_ms).sum();
        Schedule { programs: Arc::new(programs), style, seed, epoch_ms, pass_ms, cache: Arc::default() }
    }

    pub fn programs(&self) -> &[Program] {
        &self.programs
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn epoch_ms(&self) -> i64 {
        self.epoch_ms
    }

    /// How long one pass runs.
    pub fn pass_ms(&self) -> i64 {
        self.pass_ms
    }

    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }

    /// The same programmes and start, another order: what Reshuffle shows.
    pub fn reseeded(&self, seed: u64) -> Schedule {
        Schedule { seed, cache: Arc::default(), ..self.clone() }
    }

    pub fn restyled(&self, style: Style) -> Schedule {
        Schedule { style, cache: Arc::default(), ..self.clone() }
    }

    /// The same order, started at another moment.
    pub fn with_epoch(&self, epoch_ms: i64) -> Schedule {
        Schedule { epoch_ms, ..self.clone() }
    }

    /// The order of pass `pass`, as indices into [`Self::programs`].
    pub fn pass_order(&self, pass: i64) -> Arc<Vec<usize>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, o)) = cache.iter().find(|(p, _)| *p == pass) {
            return Arc::clone(o);
        }
        let o = Arc::new(order(&self.programs, self.style, mix(self.seed, pass as u64)));
        if cache.len() >= CACHED_PASSES {
            cache.remove(0);
        }
        cache.push((pass, Arc::clone(&o)));
        o
    }

    /// What is on at `t`.
    pub fn at(&self, t: i64) -> Option<Slot> {
        if self.pass_ms <= 0 {
            return None;
        }
        let since = t - self.epoch_ms;
        let pass = since.div_euclid(self.pass_ms);
        let mut start = self.epoch_ms + pass * self.pass_ms;
        for (pos, &program) in self.pass_order(pass).iter().enumerate() {
            let stop = start + self.programs[program].dur_ms;
            if t < stop {
                return Some(Slot { program, start_ms: start, stop_ms: stop, pass, pos });
            }
            start = stop;
        }
        None
    }

    /// The slot that follows `slot`.
    pub fn after(&self, slot: &Slot) -> Option<Slot> {
        self.at(slot.stop_ms)
    }

    /// Every slot that overlaps `[from, to)`, in time order. A window is walked pass by pass, so
    /// a day of guide over a long channel costs one order per pass it touches.
    pub fn window(&self, from: i64, to: i64) -> Vec<Slot> {
        let mut out = Vec::new();
        let Some(first) = self.at(from) else { return out };
        let mut pass = first.pass;
        let mut start = self.epoch_ms + pass * self.pass_ms;
        'passes: while start < to {
            for (pos, &program) in self.pass_order(pass).iter().enumerate() {
                let stop = start + self.programs[program].dur_ms;
                if stop > from && start < to {
                    out.push(Slot { program, start_ms: start, stop_ms: stop, pass, pos });
                }
                start = stop;
                if start >= to {
                    break 'passes;
                }
            }
            pass += 1;
        }
        out
    }
}

/// A starting moment that lands `now` at a random point of a random programme: the channel is
/// already "on the air" when it is first tuned, like one switched to mid-evening.
pub fn epoch_for_random_join(pass_ms: i64, now_ms: i64, seed: u64) -> i64 {
    if pass_ms <= 0 {
        return now_ms;
    }
    now_ms - (mix(seed, 0x6a6f_696e) % pass_ms as u64) as i64
}

/// SplitMix64: a fast, well-mixed, dependency-free PRNG — the same sequence on every build and
/// every television, which is the property a shared timeline needs.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

/// Two values mixed into one seed.
pub fn mix(a: u64, b: u64) -> u64 {
    let mut r = Rng::new(a ^ b.rotate_left(32) ^ 0xA076_1D64_78BD_642F);
    r.next_u64()
}

/// A string folded into a seed (FNV-1a): stable across builds, unlike `std`'s hasher.
pub fn seed_of(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The groups of `programs` (an episode's show, or a lone film), each in natural order, the groups
/// in the order they first appear.
fn groups(programs: &[Program]) -> Vec<Vec<usize>> {
    let mut keys: Vec<&str> = Vec::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for (i, p) in programs.iter().enumerate() {
        match keys.iter().position(|k| *k == p.group()) {
            Some(g) => out[g].push(i),
            None => {
                keys.push(p.group());
                out.push(vec![i]);
            }
        }
    }
    for g in &mut out {
        g.sort_by(|&a, &b| programs[a].natural_key().partial_cmp(&programs[b].natural_key()).unwrap_or(std::cmp::Ordering::Equal));
    }
    out
}

fn order(programs: &[Program], style: Style, seed: u64) -> Vec<usize> {
    let mut rng = Rng::new(seed);
    match style {
        Style::InOrder => (0..programs.len()).collect(),
        Style::Random => {
            let mut v: Vec<usize> = (0..programs.len()).collect();
            rng.shuffle(&mut v);
            let keyed: Vec<(&str, usize)> = v.into_iter().map(|i| (programs[i].spread_key(), i)).collect();
            spread(keyed)
        }
        Style::RoundRobin => {
            let mut gs = groups(programs);
            // Lone films ride as one shuffled group, so a mixed channel rotates shows and "a film".
            let (films, mut shows): (Vec<Vec<usize>>, Vec<Vec<usize>>) =
                gs.drain(..).partition(|g| g.len() == 1 && !programs[g[0]].episode);
            let mut films: Vec<usize> = films.into_iter().flatten().collect();
            rng.shuffle(&mut films);
            if !films.is_empty() {
                shows.push(films);
            }
            rng.shuffle(&mut shows);
            let mut out = Vec::with_capacity(programs.len());
            let mut cursor = vec![0usize; shows.len()];
            while out.len() < programs.len() {
                for (g, show) in shows.iter().enumerate() {
                    if let Some(&p) = show.get(cursor[g]) {
                        out.push(p);
                        cursor[g] += 1;
                    }
                }
            }
            out
        }
        Style::Blocks => {
            let mut blocks: Vec<Vec<usize>> = Vec::new();
            for g in groups(programs) {
                let size = 2 + rng.below(2); // two or three, chosen per show per pass
                for chunk in g.chunks(size) {
                    blocks.push(chunk.to_vec());
                }
            }
            rng.shuffle(&mut blocks);
            let keyed: Vec<(&str, Vec<usize>)> =
                blocks.into_iter().map(|b| (programs[b[0]].spread_key(), b)).collect();
            spread(keyed).into_iter().flatten().collect()
        }
    }
}

/// Keep the same show off consecutive slots wherever the mix allows. Items arrive already shuffled
/// with their group; the result takes, at each step, the earliest remaining item whose group is not
/// the one just placed — unless one group holds more than half of what remains, which must then go
/// next or it could never be spread at all. Deterministic for a given input order, so the timeline
/// stays a pure function of the recipe. Films carry their own key and never collide.
fn spread<T>(items: Vec<(&str, T)>) -> Vec<T> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (g, _) in &items {
        match counts.iter_mut().find(|(k, _)| k == g) {
            Some((_, n)) => *n += 1,
            None => counts.push((g, 1)),
        }
    }
    let mut rest: Vec<Option<(&str, T)>> = items.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(rest.len());
    let mut last: Option<&str> = None;
    let mut remaining = rest.len();
    let mut head = 0;
    while remaining > 0 {
        while rest[head].is_none() {
            head += 1;
        }
        let forced = counts.iter().find(|(g, n)| *n * 2 > remaining + 1 && Some(*g) != last).map(|(g, _)| *g);
        let pick = rest[head..].iter().position(|x| match (x, forced) {
            (Some((g, _)), Some(f)) => *g == f,
            (Some((g, _)), None) => Some(*g) != last,
            (None, _) => false,
        });
        let at = pick.map_or(head, |i| head + i);
        let (g, item) = rest[at].take().expect("picked a present item");
        if let Some((_, n)) = counts.iter_mut().find(|(k, _)| *k == g) {
            *n -= 1;
        }
        last = Some(g);
        out.push(item);
        remaining -= 1;
    }
    out
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
