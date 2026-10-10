//! **On Now** — what Home's live shelf shows: one card per channel, the programme airing on it,
//! in the order a viewer reaches for them.
//!
//! The order is the whole design. The channel last tuned (`Session::livetv_channel`, the one the
//! guide opens on) comes first, because the likeliest live press from Home is "back to what I was
//! watching"; every other channel follows in lineup order (channel-number order, `guide::Lineup`).
//! The shelf is capped at [`ON_NOW_MAX`]: Home is a front door, the guide is where a long lineup is
//! browsed.
//!
//! A card's artwork is the airing's own icon when the guide has one, else the channel's logo, else
//! nothing (Home draws a tile naming the channel). A channel with nothing listed right now still
//! has a card — it tunes — and reads as its name with no programme and no progress.
//!
//! Pure over the lineup and a wall-clock time, so the ordering and the minute-by-minute progress
//! are graded on the host; Home rebuilds the cards when the guide or the minute moves.

use super::guide::Lineup;

/// Cards the shelf holds at most.
pub const ON_NOW_MAX: usize = 12;

/// One channel as the On Now shelf shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnNowCard {
    /// The channel's index in the lineup it was built from — what `LiveTvReq::Tune` takes.
    pub index: usize,
    /// As the device spells it (`GuideNumber`): the card's identity and what a tune is keyed on.
    pub number: String,
    pub name: String,
    /// The programme on now; empty when the guide lists nothing at this time.
    pub title: String,
    /// The airing's icon, else the channel's logo, else empty. An absolute URL on the guide's host.
    pub art: String,
    /// The airing's span (wall-clock ms); both 0 when nothing is listed.
    pub start_ms: i64,
    pub stop_ms: i64,
}

impl OnNowCard {
    /// `"12 Films"` — the channel as a viewer names it.
    pub fn channel_label(&self) -> String {
        match (self.number.is_empty(), self.name.is_empty()) {
            (false, false) => format!("{} {}", self.number, self.name),
            (false, true) => self.number.clone(),
            _ => self.name.clone(),
        }
    }
}

/// The cards for `lineup` at `now_ms`, the channel numbered `last` first (when the lineup has it),
/// then the rest in lineup order, at most `cap`.
pub fn on_now(lineup: &Lineup, last: &str, now_ms: i64, cap: usize) -> Vec<OnNowCard> {
    let first = (!last.is_empty()).then(|| lineup.index_of_number(last)).flatten();
    first
        .into_iter()
        .chain((0..lineup.len()).filter(|&i| Some(i) != first))
        .take(cap)
        .map(|index| {
            let channel = &lineup.channels[index];
            let airing = channel.airing_at(now_ms).map(|i| &channel.airings[i]);
            let art = airing
                .map(|a| a.icon.as_str())
                .filter(|icon| !icon.is_empty())
                .unwrap_or(&channel.icon);
            OnNowCard {
                index,
                number: channel.number.clone(),
                name: channel.name.clone(),
                title: airing.map(|a| a.title.clone()).unwrap_or_default(),
                art: art.to_owned(),
                start_ms: airing.map_or(0, |a| a.start_ms),
                stop_ms: airing.map_or(0, |a| a.stop_ms),
            }
        })
        .collect()
}

/// The cards as Home's catalog rows ([`crate::pms::KIND_CHANNEL`]), for the hub store's On Now
/// shelf (`HubsCmd::SetOnNow`). `art_sid` is the server whose photo transcoder fetches the artwork
/// (an absolute URL on the guide's host, which a PMS fetches the way it fetches a credit's
/// headshot); `UNSET` when there is none, and the cards then draw their names.
///
/// The progress bar is the resume pair: `resume_ms` the time the airing has run, `dur_ns` its
/// length — so the shared resume rule ([`crate::pms::PmsMovie::resume_frac`]) draws it, and a card
/// with nothing airing has none.
pub fn rows(cards: &[OnNowCard], art_sid: plx_plex::plex::ServerId, now_ms: i64) -> Vec<crate::pms::PmsMovie> {
    cards
        .iter()
        .map(|card| {
            let length = card.stop_ms - card.start_ms;
            let airing = !card.title.is_empty() && length > 0;
            // A virtual channel's art is a path on one of the viewer's own servers
            // (`plex:<server>:<path>`); a Tunarr picture is an absolute URL the art server fetches.
            let (sid, thumb) = match card.art.strip_prefix("plex:").and_then(|r| r.split_once(':')) {
                Some((srv, path)) => (srv.parse::<u16>().map(plx_plex::plex::ServerId::from_raw).unwrap_or(art_sid), path.to_owned()),
                None => (art_sid, if art_sid.is_set() { card.art.clone() } else { String::new() }),
            };
            crate::pms::PmsMovie {
                sid,
                kind: crate::pms::KIND_CHANNEL,
                rk: card.number.clone(),
                title: if card.title.is_empty() { card.channel_label() } else { card.title.clone() },
                show_title: card.channel_label(),
                thumb,
                resume_ms: if airing { (now_ms - card.start_ms).clamp(1, length - 1) } else { 0 },
                dur_ns: if airing { length * 1_000_000 } else { 0 },
                ..Default::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::livetv::guide::{Airing, Channel};

    fn channel(number: &str, icon: &str, airings: Vec<Airing>) -> Channel {
        Channel { number: number.into(), name: format!("n{number}"), icon: icon.into(), airings, ..Default::default() }
    }

    fn airing(start: i64, stop: i64, title: &str, icon: &str) -> Airing {
        Airing { start_ms: start, stop_ms: stop, title: title.into(), icon: icon.into(), ..Default::default() }
    }

    fn lineup(n: usize) -> Lineup {
        Lineup { channels: (1..=n).map(|i| channel(&i.to_string(), "", Vec::new())).collect(), ..Default::default() }
    }

    fn numbers(cards: &[OnNowCard]) -> Vec<&str> {
        cards.iter().map(|c| c.number.as_str()).collect()
    }

    #[test]
    fn the_last_tuned_channel_leads_and_the_rest_keep_lineup_order() {
        let cards = on_now(&lineup(5), "4", 0, ON_NOW_MAX);
        assert_eq!(numbers(&cards), ["4", "1", "2", "3", "5"]);
        assert_eq!(cards[0].index, 3, "the card carries its lineup index for the tune");
    }

    #[test]
    fn an_unknown_or_unset_last_channel_changes_nothing() {
        assert_eq!(numbers(&on_now(&lineup(3), "", 0, ON_NOW_MAX)), ["1", "2", "3"]);
        assert_eq!(numbers(&on_now(&lineup(3), "77", 0, ON_NOW_MAX)), ["1", "2", "3"]);
    }

    #[test]
    fn the_shelf_is_capped_with_the_last_channel_kept() {
        let cards = on_now(&lineup(30), "25", 0, ON_NOW_MAX);
        assert_eq!(cards.len(), ON_NOW_MAX);
        assert_eq!(cards[0].number, "25", "a channel past the cap still leads when it was last tuned");
        assert_eq!(cards[ON_NOW_MAX - 1].number, "11");
    }

    #[test]
    fn a_card_shows_the_airing_on_now_and_moves_with_the_clock() {
        let l = Lineup {
            channels: vec![channel("1", "logo", vec![airing(0, 1000, "early", "show-icon"), airing(1000, 2000, "late", "")])],
            ..Default::default()
        };
        let early = &on_now(&l, "", 500, ON_NOW_MAX)[0];
        assert_eq!((early.title.as_str(), early.art.as_str()), ("early", "show-icon"), "the airing's own icon first");
        assert_eq!((early.start_ms, early.stop_ms), (0, 1000));
        let late = &on_now(&l, "", 1500, ON_NOW_MAX)[0];
        assert_eq!((late.title.as_str(), late.art.as_str()), ("late", "logo"), "else the channel's logo");
    }

    #[test]
    fn a_row_carries_the_tune_key_the_label_and_the_airing_progress() {
        let l = Lineup { channels: vec![channel("7", "logo", vec![airing(1_000, 61_000, "film", "")])], ..Default::default() };
        let sid = plx_plex::plex::ServerId::from_raw(1);
        let row = &rows(&on_now(&l, "", 31_000, ON_NOW_MAX), sid, 31_000)[0];
        assert_eq!(row.kind, crate::pms::KIND_CHANNEL);
        assert_eq!((row.rk.as_str(), row.title.as_str(), row.show_title.as_str()), ("7", "film", "7 n7"));
        assert_eq!(row.thumb, "logo");
        assert_eq!(row.resume_frac(), Some(0.5), "half of the airing has run");
    }

    #[test]
    fn a_row_with_nothing_airing_names_the_channel_and_has_no_bar() {
        let l = Lineup { channels: vec![channel("9", "logo", Vec::new())], ..Default::default() };
        let row = &rows(&on_now(&l, "", 0, ON_NOW_MAX), plx_plex::plex::ServerId::UNSET, 0)[0];
        assert_eq!(row.title, "9 n9");
        assert_eq!(row.resume_frac(), None);
        assert_eq!(row.thumb, "", "no server to fetch the artwork through: the card draws its name");
    }

    #[test]
    fn a_channel_with_nothing_on_still_has_a_card() {
        let l = Lineup { channels: vec![channel("9", "", vec![airing(5000, 6000, "later", "")])], ..Default::default() };
        let card = &on_now(&l, "", 0, ON_NOW_MAX)[0];
        assert_eq!(card.title, "", "nothing airing yet");
        assert_eq!((card.start_ms, card.stop_ms), (0, 0));
        assert_eq!(card.art, "");
        assert_eq!(card.channel_label(), "9 n9");
    }
}
