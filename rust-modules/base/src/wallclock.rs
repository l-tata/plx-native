//! **Wall time**, for the few readers that need the calendar rather than the frame clock.
//!
//! Everything that animates or times out reads the frame clock (`app::clock`, `Instant`), which is
//! monotonic and starts at boot. A TV guide cannot: an airing is "8:00 PM to 8:30 PM" on the wall,
//! and whether it is on NOW is a comparison against the epoch. This module is that door and
//! nothing else — the epoch in milliseconds, and the local UTC offset the C library would apply to
//! a given instant, so a guide can print "20:00" without each caller reaching into `libc`.
//!
//! The offset is asked per instant (`localtime_r`'s `tm_gmtoff`) rather than cached, because a
//! daylight-saving change inside a guide's window moves it, and an airing at 01:30 on the night
//! the clocks go back is printed with the offset of THAT instant.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The two stand-ins for a zone the C library cannot see; `NO_HINT` when absent.
static SYSTEM_OFFSET: AtomicI64 = AtomicI64::new(NO_HINT);
static GUIDE_OFFSET: AtomicI64 = AtomicI64::new(NO_HINT);
const NO_HINT: i64 = i64::MIN;

/// The set's own offset, from its system clock service (`tv::system_utc_offset_s`, asked once at
/// boot).
///
/// A native process on the television may not see the zone the set is configured for: the jailed
/// app's `localtime` can fall back to UTC (the device's own pmlog reads hours off). The system
/// service answers with the zone the owner chose, so it outranks every other stand-in; it is used
/// only while `localtime` says UTC, so a process that sees its zone keeps it.
pub fn set_system_offset(offset_s: Option<i32>) {
    SYSTEM_OFFSET.store(offset_s.map_or(NO_HINT, i64::from), Ordering::Relaxed);
}

/// The zone the Live TV guide's server writes XMLTV in — for a household's own Tunarr, the
/// household's zone. The last resort: used only while `localtime` says UTC and the set's system
/// service gave nothing.
pub fn set_offset_hint(offset_s: Option<i32>) {
    GUIDE_OFFSET.store(offset_s.map_or(NO_HINT, i64::from), Ordering::Relaxed);
}

fn load(hint: &AtomicI64) -> Option<i32> {
    let v = hint.load(Ordering::Relaxed);
    (v != NO_HINT).then(|| i32::try_from(v).unwrap_or(0))
}

/// The C library's offset, unless it is UTC and a stand-in says otherwise: the system service's
/// first, then the guide's. Pure: the decision [`local_offset_s`] makes.
pub fn effective_offset(libc_offset_s: i32, system: Option<i32>, guide: Option<i32>) -> i32 {
    if libc_offset_s != 0 {
        return libc_offset_s;
    }
    system.or(guide).unwrap_or(0)
}

/// Milliseconds since the Unix epoch, or 0 if the clock reads before 1970 (an unset TV clock).
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The local UTC offset, in seconds east of Greenwich, at the instant `epoch_ms` — the C library's,
/// or a stand-in ([`set_system_offset`], then [`set_offset_hint`]) when the library says UTC.
///
/// 0 when the C library cannot convert the instant (out of range for `time_t`) and there is no
/// hint, which prints UTC rather than nonsense.
pub fn local_offset_s(epoch_ms: i64) -> i32 {
    effective_offset(libc_offset_s(epoch_ms), load(&SYSTEM_OFFSET), load(&GUIDE_OFFSET))
}

fn libc_offset_s(epoch_ms: i64) -> i32 {
    let secs = epoch_ms.div_euclid(1000);
    // `.ok()`: `time_t` is 32 bits on the television and 64 on a host, so the conversion is
    // fallible on one and infallible on the other.
    let Some(t) = libc::time_t::try_from(secs).ok() else { return 0 };
    // SAFETY: `localtime_r` writes only into the `tm` we hand it and reads only `t`; a zeroed `tm`
    // is a valid out-parameter.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        i32::try_from(tm.tm_gmtoff).unwrap_or(0)
    }
}

/// The local wall-clock hour (0-23) and minute at `epoch_ms`.
pub fn local_hm(epoch_ms: i64) -> (u32, u32) {
    hm_at_offset(epoch_ms, local_offset_s(epoch_ms))
}

/// The hour and minute of `epoch_ms` shifted by `offset_s`. Pure: [`local_hm`] supplies the offset.
pub fn hm_at_offset(epoch_ms: i64, offset_s: i32) -> (u32, u32) {
    let local_s = epoch_ms.div_euclid(1000) + i64::from(offset_s);
    let of_day = local_s.rem_euclid(86_400);
    ((of_day / 3600) as u32, ((of_day % 3600) / 60) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hour_and_minute_follow_the_offset_and_wrap_the_day() {
        // 2026-10-09T00:30:00Z
        let t = 1_791_505_800_000;
        assert_eq!(hm_at_offset(t, 0), (0, 30));
        assert_eq!(hm_at_offset(t, -4 * 3600), (20, 30), "a western offset is the previous evening");
        assert_eq!(hm_at_offset(t, 5 * 3600 + 1800), (6, 0), "a half-hour zone");
        assert_eq!(hm_at_offset(-1, 0), (23, 59), "an instant before the epoch still has a time of day");
    }

    #[test]
    fn a_stand_in_is_used_only_for_a_library_that_says_utc() {
        assert_eq!(effective_offset(0, None, Some(-5 * 3600)), -5 * 3600, "the guide's zone, last");
        assert_eq!(effective_offset(0, Some(3600), Some(-5 * 3600)), 3600, "the set's system zone outranks the guide's");
        assert_eq!(effective_offset(7200, Some(3600), Some(-5 * 3600)), 7200, "a process that sees its zone keeps it");
        assert_eq!(effective_offset(0, None, None), 0);
    }

    #[test]
    fn the_clock_reads_after_the_epoch() {
        assert!(now_ms() > 0);
    }
}
