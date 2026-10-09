//! **The joined guide**: the lineup's channels (what can be tuned) with the XMLTV airings that
//! belong to each (what is on). The one model every Live TV surface reads.
//!
//! The join is on the CHANNEL NUMBER, the way HDHomeRun clients join a lineup to an XMLTV file:
//! `lineup.json`'s `GuideNumber` is Tunarr's `channel.number`, and Tunarr's `<channel>` carries the
//! same number as a bare `<display-name>`. The XMLTV `id` scheme (`C{n}.{code}.tunarr.com`) is
//! deliberately not rebuilt here — it is Tunarr's private spelling, and the number is the
//! contract.

use super::hdhr::LineupEntry;
use super::xmltv::{Guide, GuideChannel, Programme};

/// One airing on one channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Airing {
    pub start_ms: i64,
    /// Always after `start_ms` (an open-ended XMLTV airing is closed by the next one).
    pub stop_ms: i64,
    pub title: String,
    pub sub_title: String,
    pub desc: String,
    pub episode: String,
    pub category: String,
    pub icon: String,
}

impl Airing {
    /// Is `t` inside this airing?
    pub fn covers(&self, t: i64) -> bool {
        self.start_ms <= t && t < self.stop_ms
    }

    /// How far through the airing `t` is, 0..=1.
    pub fn progress(&self, t: i64) -> f32 {
        let span = (self.stop_ms - self.start_ms).max(1) as f64;
        (((t - self.start_ms) as f64 / span).clamp(0.0, 1.0)) as f32
    }
}

/// One tunable channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    /// As the device spells it (`GuideNumber`).
    pub number: String,
    pub name: String,
    /// The channel's continuous MPEG-TS stream.
    pub url: String,
    /// Logo URL from the guide, empty when the guide has none.
    pub icon: String,
    /// Sorted by start, non-overlapping.
    pub airings: Vec<Airing>,
}

impl Channel {
    /// The airing on at `t`, by index.
    pub fn airing_at(&self, t: i64) -> Option<usize> {
        let i = self.airings.partition_point(|a| a.stop_ms <= t);
        self.airings.get(i).filter(|a| a.covers(t)).map(|_| i)
    }

    /// What is on at `t`, and what follows it (the next airing to START after `t` when nothing is
    /// on, so a gap still shows what is coming).
    pub fn now_next(&self, t: i64) -> (Option<&Airing>, Option<&Airing>) {
        match self.airing_at(t) {
            Some(i) => (self.airings.get(i), self.airings.get(i + 1)),
            None => (None, self.airings.iter().find(|a| a.start_ms > t)),
        }
    }
}

/// The whole guide: every tunable channel, in channel-number order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lineup {
    pub channels: Vec<Channel>,
    /// The guide's own UTC offset ([`Guide::source_offset_s`]), a hint for printing wall times on
    /// a television whose C library does not know its zone.
    pub source_offset_s: Option<i32>,
}

/// The length an airing is given when the guide says nothing about where it ends and nothing
/// follows it.
const OPEN_ENDED_MS: i64 = 30 * 60 * 1000;

impl Lineup {
    /// Join `entries` to `guide` (absent when the guide could not be fetched: the channels still
    /// tune, they just have nothing listed).
    pub fn join(entries: Vec<LineupEntry>, guide: Option<&Guide>) -> Lineup {
        let mut channels: Vec<Channel> = entries
            .into_iter()
            .map(|e| {
                let listed = guide.and_then(|g| guide_channel_for(&g.channels, &e.number, &e.name));
                let airings = match (guide, listed) {
                    (Some(g), Some(c)) => airings_of(&g.programmes, &c.id),
                    _ => Vec::new(),
                };
                Channel {
                    icon: listed.map(|c| c.icon.clone()).unwrap_or_default(),
                    number: e.number,
                    name: e.name,
                    url: e.url,
                    airings,
                }
            })
            .collect();
        channels.sort_by(|a, b| number_order(&a.number, &b.number));
        Lineup { channels, source_offset_s: guide.and_then(|g| g.source_offset_s) }
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    /// The channel whose number is exactly `number`.
    pub fn index_of_number(&self, number: &str) -> Option<usize> {
        self.channels.iter().position(|c| c.number == number)
    }

    /// The channel `delta` steps from `from`, wrapping at both ends (CH▲ on the last channel is
    /// the first).
    pub fn step(&self, from: usize, delta: i32) -> Option<usize> {
        let n = i64::try_from(self.channels.len()).ok().filter(|&n| n > 0)?;
        Some((i64::try_from(from).ok()? + i64::from(delta)).rem_euclid(n) as usize)
    }

    /// The channel a typed number tunes: an exact match, else the nearest channel at or above it
    /// (typing `7` on a lineup of 5, 8, 9 lands on 8, the way a TV does).
    pub fn index_for_typed(&self, typed: &str) -> Option<usize> {
        if let Some(i) = self.index_of_number(typed) {
            return Some(i);
        }
        let want = number_key(typed)?;
        self.channels.iter().position(|c| number_key(&c.number).is_some_and(|k| k >= want))
    }
}

/// The guide channel for a lineup entry: the one with a bare display name equal to the number,
/// else one whose first word is the number, else one named exactly like the entry.
fn guide_channel_for<'a>(channels: &'a [GuideChannel], number: &str, name: &str) -> Option<&'a GuideChannel> {
    channels
        .iter()
        .find(|c| c.display_names.iter().any(|d| d == number))
        .or_else(|| channels.iter().find(|c| c.display_names.iter().any(|d| d.split_whitespace().next() == Some(number))))
        .or_else(|| channels.iter().find(|c| c.display_names.iter().any(|d| d == name)))
}

fn airings_of(programmes: &[Programme], channel_id: &str) -> Vec<Airing> {
    let mut mine: Vec<&Programme> = programmes.iter().filter(|p| p.channel == channel_id).collect();
    mine.sort_by_key(|p| p.start_ms);
    let mut out: Vec<Airing> = Vec::with_capacity(mine.len());
    for (i, p) in mine.iter().enumerate() {
        let next_start = mine.get(i + 1).map(|n| n.start_ms);
        let mut stop = p.stop_ms;
        if stop <= p.start_ms {
            stop = next_start.filter(|&n| n > p.start_ms).unwrap_or(p.start_ms + OPEN_ENDED_MS);
        }
        // An airing may not run into the next one: the guide draws them side by side.
        if let Some(n) = next_start.filter(|&n| n > p.start_ms) {
            stop = stop.min(n);
        }
        // Drop a duplicate start (two programmes at one instant): the first wins.
        if out.last().is_some_and(|last: &Airing| last.start_ms == p.start_ms) {
            continue;
        }
        out.push(Airing {
            start_ms: p.start_ms,
            stop_ms: stop,
            title: p.title.clone(),
            sub_title: p.sub_title.clone(),
            desc: p.desc.clone(),
            episode: p.episode.clone(),
            category: p.category.clone(),
            icon: p.icon.clone(),
        });
    }
    out
}

/// A channel number as a sortable key: `5.1` → (5, 1), `12` → (12, 0). `None` for a number that is
/// not one (a device may name a channel anything).
fn number_key(n: &str) -> Option<(u64, u64)> {
    let (major, minor) = n.trim().split_once(['.', '-']).unwrap_or((n.trim(), "0"));
    Some((major.parse().ok()?, if minor.is_empty() { 0 } else { minor.parse().ok()? }))
}

fn number_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (number_key(a), number_key(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: &str, name: &str) -> LineupEntry {
        LineupEntry { number: n.into(), name: name.into(), url: format!("http://t/stream/channels/{n}.ts") }
    }

    fn prog(ch: &str, start: i64, stop: i64, title: &str) -> Programme {
        Programme { channel: ch.into(), start_ms: start, stop_ms: stop, title: title.into(), ..Default::default() }
    }

    fn guide() -> Guide {
        Guide {
            channels: vec![
                GuideChannel { id: "C1.x".into(), display_names: vec!["1 Cartoons".into(), "1".into(), "Cartoons".into()], icon: "logo1".into() },
                GuideChannel { id: "C12.x".into(), display_names: vec!["12 Films".into()], icon: String::new() },
            ],
            programmes: vec![
                prog("C1.x", 2000, 3000, "second"),
                prog("C1.x", 1000, 2000, "first"),
                prog("C1.x", 3000, 3000, "open-ended"),
                prog("C12.x", 0, 5000, "long"),
                prog("C99.x", 0, 5000, "nobody's"),
            ],
            source_offset_s: Some(-4 * 3600),
        }
    }

    #[test]
    fn channels_join_by_number_and_sort_numerically() {
        let l = Lineup::join(vec![entry("12", "Films"), entry("2", "Unlisted"), entry("1", "Cartoons")], Some(&guide()));
        assert_eq!(l.source_offset_s, Some(-4 * 3600), "the guide's zone rides along");
        assert_eq!(l.channels.iter().map(|c| c.number.as_str()).collect::<Vec<_>>(), ["1", "2", "12"]);
        assert_eq!(l.channels[0].icon, "logo1");
        assert_eq!(l.channels[0].airings.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["first", "second", "open-ended"]);
        assert_eq!(l.channels[0].airings[2].stop_ms, 3000 + OPEN_ENDED_MS, "an open end with nothing after it gets the default length");
        assert!(l.channels[1].airings.is_empty(), "a channel the guide does not list still tunes");
        assert_eq!(l.channels[2].airings[0].title, "long", "joined by the first word of `12 Films`");
    }

    #[test]
    fn no_guide_still_gives_tunable_channels() {
        let l = Lineup::join(vec![entry("1", "A")], None);
        assert_eq!(l.len(), 1);
        assert!(l.channels[0].airings.is_empty());
    }

    #[test]
    fn now_and_next_follow_the_clock() {
        let l = Lineup::join(vec![entry("1", "Cartoons")], Some(&guide()));
        let c = &l.channels[0];
        assert_eq!(c.airing_at(999), None);
        assert_eq!(c.airing_at(1000), Some(0));
        assert_eq!(c.airing_at(1999), Some(0));
        assert_eq!(c.airing_at(2000), Some(1));
        let (now, next) = c.now_next(1500);
        assert_eq!((now.unwrap().title.as_str(), next.unwrap().title.as_str()), ("first", "second"));
        let (now, next) = c.now_next(500);
        assert!(now.is_none());
        assert_eq!(next.unwrap().title, "first", "a gap shows what is coming");
        assert!((c.airings[0].progress(1500) - 0.5).abs() < 1e-6);
        assert_eq!(c.airings[0].progress(0), 0.0);
        assert_eq!(c.airings[0].progress(9999), 1.0);
    }

    #[test]
    fn overlapping_airings_are_cut_at_the_next_start() {
        let g = Guide {
            channels: vec![GuideChannel { id: "c".into(), display_names: vec!["1".into()], icon: String::new() }],
            programmes: vec![prog("c", 0, 5000, "a"), prog("c", 3000, 6000, "b"), prog("c", 3000, 4000, "dup")],
            source_offset_s: None,
        };
        let l = Lineup::join(vec![entry("1", "x")], Some(&g));
        let a = &l.channels[0].airings;
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].start_ms, a[0].stop_ms), (0, 3000));
        assert_eq!(a[1].title, "b");
    }

    #[test]
    fn stepping_wraps_and_typing_lands_on_the_nearest_channel() {
        let l = Lineup::join(vec![entry("5", "a"), entry("8", "b"), entry("9", "c"), entry("9.1", "d")], None);
        assert_eq!(l.step(0, -1), Some(3));
        assert_eq!(l.step(3, 1), Some(0));
        assert_eq!(l.step(1, 1), Some(2));
        assert_eq!(l.index_for_typed("8"), Some(1));
        assert_eq!(l.index_for_typed("7"), Some(1));
        assert_eq!(l.index_for_typed("9.1"), Some(3));
        assert_eq!(l.index_for_typed("10"), None);
        assert_eq!(Lineup::default().step(0, 1), None);
    }
}
