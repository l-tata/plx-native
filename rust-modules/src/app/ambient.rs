//! **The ambient screensaver**: a parked television fades to a slow slideshow of the library's art.
//!
//! webOS does not blank or dim a foreground native app (`boot.rs`, the measured screensaver dead
//! end), so before this a TV left on Home showed the same bright chrome all night: wasted light, and
//! the static bar and hero are exactly what burns into an OLED panel. After the delay chosen in
//! Settings > Playback > Screensaver (five minutes unless changed; Off turns it off) with no
//! input on a browsing page (and never over video), the screen fades to full-bleed backdrops from
//! the Home hubs. Each one zooms and pans slowly (Ken Burns), crossfades into the next, and carries
//! its title and the wall clock. The text block shifts a little on every slide so no pixel holds a
//! glyph for long.
//!
//! The first key only wakes the page. [`swallow_key`] eats that press, its repeats and its key-up,
//! so waking the TV never also activates the focused control. Pointer motion wakes it through
//! `last_input`, like any other input.

use plx_ui::{theme, Painter, Rect};
use plx_screens::registry::AppArg;

use super::App;

#[cfg(test)]
const IDLE_MS: u32 = 5 * 60 * 1000;
/// How long each slide is on screen, its crossfade included.
const SLIDE_MS: u32 = 12_000;
/// The crossfade from one slide to the next, at the end of each slide.
const FADE_MS: u32 = 2_000;
/// The fade from the page into ambient.
const ENTER_MS: u32 = 1_500;
/// At most this many slides are gathered from the hubs.
const MAX_SLIDES: usize = 30;
/// A slide is presented at 30 frames per second: the motion is slow enough that 60 buys nothing
/// a viewer can see, and a parked TV should not run its GPU flat out.
const FRAME_MS: u32 = 33;

/// One backdrop and what it is captioned with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slide {
    sid: u16,
    art: String,
    title: String,
    sub: String,
}

#[derive(Default)]
pub(crate) struct Ambient {
    /// When ambient began (`clock::now`), while it is up.
    since: Option<u32>,
    /// `App::last_input` when ambient began: input after that wakes the page.
    input_at: u32,
    slides: Vec<Slide>,
    /// The key whose press woke the page, swallowed until it comes up.
    swallowing: Option<u32>,
    last_frame: u32,
    /// A dev build's `ambient` trigger, read once: the idle delay in seconds, overriding the
    /// setting. `Some(None)` when no trigger is armed.
    dev_idle_ms: Option<Option<u32>>,
}

impl Ambient {
    pub(crate) fn active(&self) -> bool {
        self.since.is_some()
    }
}

/// Whether a page is one ambient may cover: a browsing page, never sign-in, profiles, consent,
/// settings or the player.
fn eligible_route(route: &AppArg) -> bool {
    matches!(route, AppArg::Home | AppArg::Library | AppArg::Search | AppArg::LiveTv | AppArg::Content(_))
}

/// Whether the page has been idle long enough.
fn idle_due(now: u32, last_input: u32, idle_ms: u32) -> bool {
    now.wrapping_sub(last_input) >= idle_ms && now.wrapping_sub(last_input) < u32::MAX / 2
}

/// Which slide is up `elapsed` ms into ambient, which one follows it, and how far the crossfade
/// to the follower has gone (0 until the last [`FADE_MS`] of the slide).
fn slide_at(elapsed: u32, n: usize) -> (usize, usize, f32) {
    if n == 0 {
        return (0, 0, 0.0);
    }
    let step = (elapsed / SLIDE_MS) as usize;
    let into = elapsed % SLIDE_MS;
    let fade = into.saturating_sub(SLIDE_MS - FADE_MS) as f32 / FADE_MS as f32;
    (step % n, (step + 1) % n, fade.clamp(0.0, 1.0))
}

/// The art's rect `t` (0..1) through its time on screen: it covers the screen, grows from 104 % to
/// 114 % and drifts toward one of four corners, chosen by the slide's position in the show.
fn ken_burns(tw: f32, th: f32, step: usize, t: f32) -> Rect {
    let t = t.clamp(0.0, 1.0);
    let base = Rect::FULL.cover(tw, th);
    let s = 1.04 + 0.10 * t;
    let (w, h) = (base.w * s, base.h * s);
    let (sx, sy) = match step % 4 {
        0 => (-1.0, -1.0),
        1 => (1.0, -1.0),
        2 => (1.0, 1.0),
        _ => (-1.0, 1.0),
    };
    let (slack_x, slack_y) = ((w - Rect::FULL.w) * 0.5, (h - Rect::FULL.h) * 0.5);
    Rect::new(
        (Rect::FULL.w - w) * 0.5 + sx * slack_x * (t - 0.5),
        (Rect::FULL.h - h) * 0.5 + sy * slack_y * (t - 0.5),
        w,
        h,
    )
}

/// How far the caption block is shifted for a slide: a few dozen pixels, never the same twice in a
/// row, so the clock's digits do not sit on one set of pixels all night.
fn orbit(step: usize) -> (f32, f32) {
    const PATH: [(f32, f32); 6] = [(0.0, 0.0), (28.0, -14.0), (12.0, -30.0), (-6.0, -10.0), (22.0, 6.0), (6.0, -22.0)];
    PATH[step % PATH.len()]
}

/// The slides, from the Home hubs' heroes and then their shelves: every distinct backdrop, in
/// order, up to [`MAX_SLIDES`].
fn collect(view: plx_data::pms::HubsView<'_>) -> Vec<Slide> {
    let mut out: Vec<Slide> = Vec::new();
    let mut push = |m: &plx_data::pms::PmsMovie| {
        if out.len() >= MAX_SLIDES || m.art.is_empty() || out.iter().any(|s| s.art == m.art) {
            return;
        }
        let (title, sub) = if m.kind == 3 && !m.show_title.is_empty() {
            (m.show_title.clone(), m.title.clone())
        } else if m.year > 0 {
            (m.title.clone(), m.year.to_string())
        } else {
            (m.title.clone(), String::new())
        };
        out.push(Slide { sid: m.sid.raw(), art: m.art.clone(), title, sub });
    };
    for i in 0..view.hero_count() {
        if let Some(h) = view.hero(i) {
            push(h.item);
        }
    }
    for i in 0..view.hub_count() {
        if let Some(hub) = view.hub(i) {
            for m in hub.items {
                push(m);
            }
        }
    }
    out
}

/// Eat a key that wakes the page from ambient: its press, its repeats and its key-up. `true` when
/// the event was ambient's and nothing else may read it.
pub(crate) fn swallow_key(app: &mut App, sym: u32, down: bool) -> bool {
    if let Some(held) = app.ambient.swallowing {
        if held == sym {
            if !down {
                app.ambient.swallowing = None;
            }
            return true;
        }
    }
    if down && app.ambient.active() {
        wake(app);
        app.ambient.swallowing = Some(sym);
        return true;
    }
    false
}

fn wake(app: &mut App) {
    app.ambient.since = None;
    app.ambient.slides.clear();
    app.last_input = super::clock::now();
    plx_machine::idle::invalidate();
    plx_base::eventlog::log("ambient: woke");
}

/// Per frame: start ambient once the page has been idle long enough, end it on input or when the
/// page stops being one it may cover, and buy the frames its motion needs.
pub(crate) fn pump(app: &mut App, now: u32, player: bool) {
    let dev = *app.ambient.dev_idle_ms.get_or_insert_with(|| {
        plx_base::devtrig::read("ambient")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(|s| s.saturating_mul(1000))
    });
    let idle_ms = dev.or_else(|| plx_media::route::screensaver().minutes().map(|m| m * 60_000));
    let covers = !player && !app.player.video_plane_bound && eligible_route(&app.route());
    match app.ambient.since {
        None => {
            if covers && idle_ms.is_some_and(|ms| idle_due(now, app.last_input, ms)) {
                app.ambient.slides = collect(app.bridge.hubs_snapshot().view());
                app.ambient.since = Some(now);
                app.ambient.input_at = app.last_input;
                app.ambient.last_frame = 0;
                plx_machine::idle::invalidate();
                plx_base::eventlog::log(&format!("ambient: on slides={}", app.ambient.slides.len()));
            }
        }
        Some(_) => {
            if !covers || app.last_input != app.ambient.input_at {
                wake(app);
                return;
            }
            if now.wrapping_sub(app.ambient.last_frame) >= FRAME_MS {
                app.ambient.last_frame = now;
                plx_machine::idle::invalidate();
            }
        }
    }
    // Hold the next slide's art warm so the crossfade never lands on an empty texture.
    if let Some(since) = app.ambient.since {
        let (_, next, _) = slide_at(now.wrapping_sub(since), app.ambient.slides.len());
        if let Some(s) = app.ambient.slides.get(next) {
            let _ = plx_ui::widgets::warm_tex_on(s.sid, &s.art, 1280, 720, 0);
        }
    }
}

/// Draw ambient over everything on a browsing page. Nothing while it is down.
pub(crate) fn draw(app: &App, now: u32) {
    let Some(since) = app.ambient.since else { return };
    let elapsed = now.wrapping_sub(since);
    let enter = (elapsed as f32 / ENTER_MS as f32).clamp(0.0, 1.0);
    let p = Painter::root().alpha(enter * enter * (3.0 - 2.0 * enter));
    p.rrect(Rect::FULL, 0.0, 0.0, theme::scrim_black(1.0));
    let slides = &app.ambient.slides;
    let (cur, next, fade) = slide_at(elapsed, slides.len());
    let step = (elapsed / SLIDE_MS) as usize;
    let t_in = (elapsed % SLIDE_MS) as f32 / (SLIDE_MS + FADE_MS) as f32;
    let art = |s: &Slide, step: usize, t: f32, a: f32| {
        if a <= 0.01 {
            return;
        }
        let (tex, tw, th) = plx_ui::widgets::resolve_tex_wh_on(s.sid, &s.art, 1280, 720, 0);
        if tex != 0 {
            p.tex(tex, ken_burns(tw, th, step, t), 0.0, theme::with_a(theme::TINT_WHITE, a));
        }
    };
    if let Some(s) = slides.get(cur) {
        art(s, step, t_in, 1.0);
    }
    if fade > 0.0 && next != cur {
        if let Some(s) = slides.get(next) {
            // The follower starts its own move as it fades in, at the time it will have on its turn.
            art(s, step + 1, fade * FADE_MS as f32 / (SLIDE_MS + FADE_MS) as f32, fade);
        }
    }
    // A dark foot for the caption, and a light overall dim: this is a screensaver, not a poster.
    p.rrect(Rect::FULL, 0.0, 0.0, theme::scrim(0.22));
    p.rect(Rect::new(0.0, 620.0, 1920.0, 460.0), 0.0, theme::scrim(0.0), theme::scrim(0.78), 0.0);

    let (ox, oy) = orbit(step);
    let caption = if fade > 0.5 { slides.get(next) } else { slides.get(cur) };
    let cap_a = if fade > 0.0 { (fade - 0.5).abs() * 2.0 } else { 1.0 };
    if let Some(s) = caption {
        let pc = p.alpha(cap_a);
        if let Ok(title) = std::ffi::CString::new(s.title.as_str()) {
            pc.text(title.as_ptr(), 120.0 + ox, 900.0 + oy, theme::size::DISPLAY, theme::TEXT_PRIMARY, 0, 1);
        }
        if let Ok(sub) = std::ffi::CString::new(s.sub.as_str()) {
            pc.text(sub.as_ptr(), 120.0 + ox, 966.0 + oy, theme::size::BODY, theme::TEXT_SECONDARY, 0, 0);
        }
    }
    let wall = plx_base::wallclock::now_ms();
    if wall >= super::bridge::CLOCK_SET_AFTER_MS {
        if let Ok(clock) = std::ffi::CString::new(plx_ui::fmt::wall_time(wall)) {
            p.text(clock.as_ptr(), 1800.0 - ox, 930.0 + oy, theme::size::HERO, theme::TEXT_PRIMARY, 2, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_due_only_after_the_delay_and_survives_clock_wrap() {
        assert!(!idle_due(1_000, 0, IDLE_MS));
        assert!(idle_due(IDLE_MS, 0, IDLE_MS));
        assert!(idle_due(5, u32::MAX - IDLE_MS + 6, IDLE_MS));
        // An input stamped just after `now` was read is not an idle of four billion ms.
        assert!(!idle_due(10, 20, IDLE_MS));
    }

    #[test]
    fn slides_step_in_order_and_crossfade_only_at_their_end() {
        assert_eq!(slide_at(0, 3), (0, 1, 0.0));
        assert_eq!(slide_at(SLIDE_MS - FADE_MS, 3), (0, 1, 0.0));
        let (cur, next, fade) = slide_at(SLIDE_MS - FADE_MS / 2, 3);
        assert_eq!((cur, next), (0, 1));
        assert!((fade - 0.5).abs() < 1e-3);
        assert_eq!(slide_at(SLIDE_MS, 3), (1, 2, 0.0));
        assert_eq!(slide_at(3 * SLIDE_MS, 3), (0, 1, 0.0));
        assert_eq!(slide_at(SLIDE_MS * 7, 1), (0, 0, 0.0));
        assert_eq!(slide_at(123, 0), (0, 0, 0.0));
    }

    #[test]
    fn the_art_always_covers_the_screen_while_it_moves() {
        for step in 0..4 {
            for i in 0..=10 {
                let r = ken_burns(1280.0, 720.0, step, i as f32 / 10.0);
                assert!(r.x <= 0.0 && r.y <= 0.0, "{step} {i} {r:?}");
                assert!(r.x + r.w >= 1920.0 && r.y + r.h >= 1080.0, "{step} {i} {r:?}");
            }
            // …and a portrait image too.
            let r = ken_burns(720.0, 1280.0, step, 1.0);
            assert!(r.x <= 0.0 && r.y <= 0.0 && r.x + r.w >= 1920.0 && r.y + r.h >= 1080.0);
        }
        // It moves.
        assert_ne!(ken_burns(1280.0, 720.0, 0, 0.0), ken_burns(1280.0, 720.0, 0, 1.0));
    }

    #[test]
    fn the_caption_never_stays_put_from_one_slide_to_the_next() {
        for step in 0..12 {
            assert_ne!(orbit(step), orbit(step + 1));
        }
    }

    #[test]
    fn only_browsing_pages_fade_to_ambient() {
        assert!(eligible_route(&AppArg::Home));
        assert!(eligible_route(&AppArg::LiveTv));
        assert!(!eligible_route(&AppArg::Player));
        assert!(!eligible_route(&AppArg::Login));
        assert!(!eligible_route(&AppArg::Profiles));
        assert!(!eligible_route(&AppArg::AccountMenu));
    }
}
