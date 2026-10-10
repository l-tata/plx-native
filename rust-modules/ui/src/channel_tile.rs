//! **A channel's tile** — the Live TV guide's channel column, the player's surf list and the guide's
//! info pane all name a channel the same way: its LOGO fitted inside a rounded plate, with its
//! number as a small badge in the corner. A channel with no logo, or one whose logo has not arrived
//! or never will, is a tile in a colour of its own ([`tint_of`], picked from its name) carrying its
//! INITIALS ([`initials`]), so a lineup without logos still reads as told-apart channels rather than
//! a column of identical grey squares.
//!
//! The logo comes through the one image pipeline ([`crate::tex`]) under [`tex::PLAIN_URL`]: a Live
//! TV server publishes its logos as plain URLs, and the store fetches, scales, caches on disk and
//! keeps them resident exactly like Plex art — which is why a revisited guide shows them at once.
//! A logo is CONTAINED, never cropped: a wordmark cut at its edges is not the channel's mark any
//! more.
//!
//! The initials tile is the ABSENT state, not a loading skeleton: it is what the channel looks like
//! when its server has no logo for it, and it is equally the right thing to show for the moment a
//! logo is on its way (a skeleton that resolves to initials when the fetch 404s would read as a
//! broken image). It is therefore not counted by `placeholder` (`CARD_ABSENT`'s argument).

use crate::label::{HAlign, Label};
use crate::{tex, theme, Painter, Rect};

/// The air between a logo and its plate's edge.
const LOGO_INSET: f32 = 6.0;
/// The badge's horizontal padding around the number, and its offset from the tile's corner.
const BADGE_PAD: f32 = 6.0;
const BADGE_INSET: f32 = 4.0;

/// **The initials a logo-less channel's tile carries.** Up to four letters, taken so a channel
/// stays recognisable: a name that is already an acronym (`HBO`, `CNN`, `ESPN2` — one word, all
/// capitals and digits, at most five characters) is kept, up to four; a name of several
/// words takes the first letter of each of its first two (`Comedy Central` → `CC`); any other
/// single word its first letter. Punctuation separates words (`Sci-Fi` → `SF`). A name with no
/// letter or digit at all gives the empty string, and the caller draws the number instead.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => {
            let acronym = one.chars().count() <= 5 && one.chars().all(|c| c.is_uppercase() || c.is_ascii_digit());
            if acronym {
                one.chars().take(4).collect()
            } else {
                one.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default()
            }
        }
        [first, second, ..] => {
            let mut out = String::new();
            for w in [first, second] {
                if let Some(c) = w.chars().next() {
                    out.extend(c.to_uppercase());
                }
            }
            out
        }
    }
}

/// **Which [`theme::CHANNEL_TILE_TINTS`] entry a logo-less channel wears**: a hash of its name
/// (FNV-1a over the lower-cased bytes), so the same channel is the same colour on every visit and
/// on every surface, and neighbouring channels are usually different ones.
pub fn tint_of(name: &str) -> usize {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.trim().to_lowercase().bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h % theme::CHANNEL_TILE_TINTS.len() as u32) as usize
}

/// The type rung a tile's initials stand at: the largest rung that leaves the tile its air.
pub fn initials_size(tile_h: f32) -> i32 {
    if tile_h >= 150.0 {
        theme::size::DISPLAY
    } else if tile_h >= 90.0 {
        theme::size::HEADLINE
    } else {
        theme::size::BODY
    }
}

/// `w`×`h` contained in `r`, centred: the rect a logo of that aspect is drawn at.
pub fn contain(r: Rect, w: f32, h: f32) -> Rect {
    if w <= 0.0 || h <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
        return r;
    }
    let k = (r.w / w).min(r.h / h);
    let (dw, dh) = (w * k, h * k);
    Rect::new(r.x + (r.w - dw) * 0.5, r.y + (r.h - dh) * 0.5, dw, dh)
}

/// One channel's tile. Built per draw from borrowed strings; it holds nothing.
pub struct ChannelTile<'a> {
    /// The channel number the badge shows (empty: no badge).
    pub number: &'a str,
    /// The channel's name — what the initials and the tint are read from.
    pub name: &'a str,
    /// The logo's absolute URL, empty when the server has none.
    pub logo: &'a str,
    badge: bool,
}

impl<'a> ChannelTile<'a> {
    pub fn new(number: &'a str, name: &'a str, logo: &'a str) -> Self {
        Self { number, name, logo, badge: true }
    }

    /// Leave the number badge off (a surface that already prints the number beside the tile).
    pub fn badge(mut self, on: bool) -> Self {
        self.badge = on;
        self
    }

    /// The logo's texture and decoded size at `r` — `(0, …)` while it is on its way, and for good
    /// when the channel has none or its fetch failed. Asks the store for exactly the box drawn, so
    /// two surfaces drawing one size share one decode.
    fn logo_tex(&self, r: Rect) -> (u32, f32, f32) {
        if self.logo.is_empty() {
            return (0, 0.0, 0.0);
        }
        { let (srv, path) = tex::art_source(self.logo); tex::resolve_wh_on(srv, path, r.w.round() as i32, r.h.round() as i32, false) }
    }

    /// The number badge's rect in a tile at `r`, and the badge's label as drawn — `None` when the
    /// tile carries no badge.
    fn badge_rect(&self, r: Rect, measure: &dyn plx_machine::machine::Measure) -> Option<(Rect, std::rc::Rc<std::ffi::CStr>)> {
        if !self.badge || self.number.is_empty() {
            return None;
        }
        let sz = theme::size::MICRO;
        let bh = measure.line_h(sz);
        let fitted = measure.fit_line(self.number, r.w - 2.0 * (BADGE_INSET + BADGE_PAD), sz, true);
        let bw = measure.width(&fitted, sz, true) + 2.0 * BADGE_PAD;
        Some((Rect::new(r.x + r.w - BADGE_INSET - bw, r.y + r.h - BADGE_INSET - bh, bw, bh), fitted))
    }

    /// Draw the tile in `r` with corner radius `rad`.
    pub fn draw(&self, p: Painter, r: Rect, rad: f32, measure: &dyn plx_machine::machine::Measure) {
        let (t, tw, th) = if p.is_recording() { (0, 0.0, 0.0) } else { self.logo_tex(r) };
        let badge = self.badge_rect(r, measure);
        if t != 0 {
            p.rect(r, rad, theme::CHANNEL_TILE_PLATE, theme::CHANNEL_TILE_PLATE, 0.0);
            let inner = Rect::new(r.x + LOGO_INSET, r.y + LOGO_INSET, r.w - 2.0 * LOGO_INSET, r.h - 2.0 * LOGO_INSET);
            p.tex(t, contain(inner, tw, th), 0.0, theme::TINT_WHITE);
        } else {
            let fill = theme::CHANNEL_TILE_TINTS[tint_of(self.name)];
            p.rect(r, rad, fill, fill, 0.0);
            let mut text = initials(self.name);
            if text.is_empty() {
                text = self.number.to_owned();
            }
            let sz = initials_size(r.h);
            let frame = initials_frame(r, badge.as_ref().map(|(b, _)| *b));
            let fitted = measure.fit_line(&text, frame.w - 2.0 * LOGO_INSET, sz, true);
            Label::new(fitted.as_ptr(), sz, theme::TEXT_PRIMARY).h(HAlign::Center).bold().draw(p, frame);
        }
        if let Some((b, label)) = badge {
            p.rect(b, (rad * 0.6).min(b.h * 0.5), theme::CHANNEL_BADGE, theme::CHANNEL_BADGE, 0.0);
            Label::new(label.as_ptr(), theme::size::MICRO, theme::TEXT_PRIMARY).h(HAlign::Center).bold().draw(p, b);
        }
    }
}

/// Where a logo-less tile's initials are centred: the whole tile, unless the tile is short enough
/// that the number badge would reach the initials' line ([`initials_size`]'s smaller rungs) — then
/// the room LEFT of the badge, so `CC` never runs into `12`.
pub fn initials_frame(r: Rect, badge: Option<Rect>) -> Rect {
    match badge {
        Some(b) if r.h < 150.0 => Rect::new(r.x, r.y, (b.x - r.x).max(0.0), r.h),
        _ => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_keep_acronyms_and_take_the_first_letters_of_words() {
        assert_eq!(initials("HBO"), "HBO");
        assert_eq!(initials("ESPN2"), "ESPN", "an acronym is kept up to four");
        assert_eq!(initials("BBCONE1"), "B", "a long run of capitals is a word, not an acronym");
        assert_eq!(initials("Comedy Central"), "CC");
        assert_eq!(initials("the movie channel"), "TM");
        assert_eq!(initials("Sci-Fi"), "SF", "punctuation separates words");
        assert_eq!(initials("Cartoons"), "C");
        assert_eq!(initials("Bluey"), "B", "a capitalised word is not an acronym");
        assert_eq!(initials("élan vital"), "ÉV");
        assert_eq!(initials("  "), "");
        assert_eq!(initials("!!"), "");
    }

    #[test]
    fn a_channel_keeps_its_tint_and_the_tints_spread() {
        assert_eq!(tint_of("Comedy Central"), tint_of("comedy central "), "case and edges do not move it");
        let names = ["News 24", "Kids", "Movies", "Sports One", "Docs", "Retro", "Music", "Weather"];
        let tints: std::collections::HashSet<usize> = names.iter().map(|n| tint_of(n)).collect();
        assert!(tints.len() >= 4, "eight names landed on {} tints", tints.len());
        assert!(names.iter().all(|n| tint_of(n) < theme::CHANNEL_TILE_TINTS.len()));
    }

    #[test]
    fn a_logo_is_contained_and_centred() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let wide = contain(r, 400.0, 100.0);
        assert_eq!((wide.x, wide.y, wide.w, wide.h), (0.0, 12.5, 100.0, 25.0));
        let tall = contain(r, 100.0, 200.0);
        assert_eq!((tall.x, tall.y, tall.w, tall.h), (37.5, 0.0, 25.0, 50.0));
        let none = contain(r, 0.0, 10.0);
        assert_eq!((none.w, none.h), (100.0, 50.0));
    }

    #[test]
    fn small_tiles_centre_their_initials_clear_of_the_badge() {
        let small = Rect::new(10.0, 0.0, 128.0, 72.0);
        let badge = Rect::new(100.0, 40.0, 34.0, 28.0);
        let f = initials_frame(small, Some(badge));
        assert_eq!((f.x, f.w, f.h), (10.0, 90.0, 72.0));
        assert_eq!(initials_frame(small, None).w, 128.0);
        let large = Rect::new(0.0, 0.0, 320.0, 180.0);
        assert_eq!(initials_frame(large, Some(badge)).w, 320.0, "a large tile keeps them centred");
    }

    #[test]
    fn initials_grow_with_the_tile() {
        assert_eq!(initials_size(64.0), theme::size::BODY);
        assert_eq!(initials_size(120.0), theme::size::HEADLINE);
        assert_eq!(initials_size(198.0), theme::size::DISPLAY);
    }
}
