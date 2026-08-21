//! The seat's wall clock: UTC seconds → local `HH:MM`, honestly.
//!
//! ★ **Why this is 120 lines instead of a dependency.** The bar needs exactly
//! one fact — the UTC offset in effect right now — and every crate that
//! provides it brings a calendar with it. `kodate` (the fleet's calendar) is
//! built on chrono; pulling chrono into a compositor to render four digits is
//! the kind of thing that later gets described as "we had to".
//!
//! TZif is pure computation over a file format, with no kernel and no
//! protocol involved, which is the one case the naturalize doctrine says to
//! rebuild rather than wrap.
//!
//! ★ **And the alternative was to keep lying.** The bar rendered `HH:MM UTC`
//! under a comment claiming it was local time. It was not — no offset was
//! ever applied. A clock that is silently three hours out is worse than no
//! clock, because it is consulted rather than ignored. So: resolve the
//! offset, or say `UTC` in `warning` and mean it.
//!
//! Reference: RFC 8536 (TZif). Only the parts needed for "what is the offset
//! now" are implemented; leap seconds, designations and the POSIX footer
//! string are read past, not interpreted.

use std::sync::OnceLock;

/// Seconds to add to UTC to get local time, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offset {
    /// Resolved from the system's zone data.
    Resolved(i32),
    /// No zone could be resolved. The caller must SAY it is UTC rather than
    /// present it as local.
    Unknown,
}

/// The offset in effect at `utc_secs`, resolved once per process.
///
/// ★ Resolved once on purpose, and that is a stated limitation rather than an
/// oversight: a seat that is up across a DST boundary will be an hour out
/// until it restarts. Re-resolving per tick would mean an open/read/parse of
/// `/etc/localtime` on the clock path, which runs on an otherwise-idle seat.
/// The honest fix when it matters is to re-resolve on the transition boundary
/// this parse already knows about — noted, not built.
#[must_use]
pub fn offset(utc_secs: i64) -> Offset {
    static CACHED: OnceLock<Offset> = OnceLock::new();
    *CACHED.get_or_init(|| resolve(utc_secs).map_or(Offset::Unknown, Offset::Resolved))
}

fn resolve(utc_secs: i64) -> Option<i32> {
    // `TZ` may name a zone (`America/Sao_Paulo`) or be empty/`UTC`.
    let path = match std::env::var("TZ") {
        Ok(tz) if !tz.is_empty() && tz != "UTC" => {
            let p = std::path::PathBuf::from("/etc/zoneinfo").join(&tz);
            if p.exists() {
                p
            } else {
                std::path::PathBuf::from("/usr/share/zoneinfo").join(&tz)
            }
        }
        _ => std::path::PathBuf::from("/etc/localtime"),
    };
    let data = std::fs::read(path).ok()?;
    parse_tzif(&data, utc_secs)
}

/// Find the UTC offset in effect at `at` in a TZif buffer.
fn parse_tzif(d: &[u8], at: i64) -> Option<i32> {
    if d.len() < 44 || &d[0..4] != b"TZif" {
        return None;
    }
    let version = d[4];

    let counts = |off: usize| -> Option<[u32; 6]> {
        let mut c = [0u32; 6];
        for (i, slot) in c.iter_mut().enumerate() {
            let s = off + i * 4;
            *slot = u32::from_be_bytes(d.get(s..s + 4)?.try_into().ok()?);
        }
        Some(c)
    };

    // v1 header is 20 bytes of magic/version/reserved then six counts.
    let [isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt] = counts(20)?;

    let v1_data = |tc: u32, ty: u32, ch: u32, lc: u32, isstd: u32, isut: u32| -> usize {
        (tc * 4 + tc + ty * 6 + ch + lc * 8 + isstd + isut) as usize
    };

    if version >= b'2' {
        // ★ USE THE SECOND BLOCK, NOT THE FIRST. The v1 block is a
        // compatibility stub with 32-bit transition times; on a zone with
        // transitions past 2038 it is both truncated and, on many systems,
        // deliberately emptied. Reading it "because it comes first" gives a
        // plausible wrong answer rather than a failure.
        let after_v1 = 44 + v1_data(timecnt, typecnt, charcnt, leapcnt, isstdcnt, isutcnt);
        let h2 = after_v1;
        if d.len() < h2 + 44 || d.get(h2..h2 + 4)? != b"TZif" {
            return None;
        }
        let [_isut2, _isstd2, _leap2, timecnt2, typecnt2, _char2] = counts(h2 + 20)?;
        let base = h2 + 44;
        return lookup64(d, base, timecnt2, typecnt2, at);
    }

    // v1-only file: 32-bit transition times.
    lookup32(d, 44, timecnt, typecnt, at)
}

fn lookup64(d: &[u8], base: usize, timecnt: u32, typecnt: u32, at: i64) -> Option<i32> {
    let tc = timecnt as usize;
    let types_at = base + tc * 8;
    let ttinfo_at = types_at + tc;
    let idx = pick(
        |i| {
            let s = base + i * 8;
            i64::from_be_bytes(d.get(s..s + 8)?.try_into().ok()?).into()
        },
        d,
        types_at,
        tc,
        at,
    );
    utoff(d, ttinfo_at, typecnt, idx)
}

fn lookup32(d: &[u8], base: usize, timecnt: u32, typecnt: u32, at: i64) -> Option<i32> {
    let tc = timecnt as usize;
    let types_at = base + tc * 4;
    let ttinfo_at = types_at + tc;
    let idx = pick(
        |i| {
            let s = base + i * 4;
            Some(i64::from(i32::from_be_bytes(
                d.get(s..s + 4)?.try_into().ok()?,
            )))
        },
        d,
        types_at,
        tc,
        at,
    );
    utoff(d, ttinfo_at, typecnt, idx)
}

/// The type index in effect at `at`: the last transition at or before it.
fn pick<F>(time_at: F, d: &[u8], types_at: usize, tc: usize, at: i64) -> usize
where
    F: Fn(usize) -> Option<i64>,
{
    let mut idx = 0usize;
    for i in 0..tc {
        match time_at(i) {
            Some(t) if t <= at => idx = usize::from(*d.get(types_at + i).unwrap_or(&0)),
            // Transitions are sorted, so the first one past `at` ends it.
            Some(_) => break,
            None => break,
        }
    }
    idx
}

fn utoff(d: &[u8], ttinfo_at: usize, typecnt: u32, idx: usize) -> Option<i32> {
    if idx >= typecnt as usize {
        return None;
    }
    let s = ttinfo_at + idx * 6;
    Some(i32::from_be_bytes(d.get(s..s + 4)?.try_into().ok()?))
}

/// `HH:MM` for a UTC timestamp, plus whether the zone was resolved.
#[must_use]
pub fn hhmm(utc_secs: i64) -> (String, bool) {
    let (secs, resolved) = match offset(utc_secs) {
        Offset::Resolved(o) => (utc_secs + i64::from(o), true),
        Offset::Unknown => (utc_secs, false),
    };
    // rem_euclid so a negative offset before the epoch still lands in [0, 86400).
    let day = secs.rem_euclid(86_400);
    let (hh, mm) = (day / 3600, (day % 3600) / 60);
    (format!("{hh:02}:{mm:02}"), resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hhmm_wraps_within_the_day() {
        // 1970-01-01T00:00Z and exactly one day later render the same.
        let (a, _) = hhmm(0);
        let (b, _) = hhmm(86_400);
        assert_eq!(a, b);
    }

    #[test]
    fn a_negative_offset_does_not_produce_a_negative_hour() {
        // ★ THE `rem_euclid` THIS PINS. `%` in Rust keeps the sign, so a UTC
        // timestamp early in the epoch plus a western offset yields a NEGATIVE
        // remainder and formats as "-3:00" — or, with the truncating cast that
        // preceded it, as "00:00" for the whole first day west of Greenwich.
        let (s, _) = hhmm(60); // 00:01Z; a -03:00 zone is 21:01 the day before
        assert!(s.len() == 5 && s.as_bytes()[2] == b':', "got {s}");
        for secs in [0_i64, 60, 3600, 86_399, -1, -3600] {
            let (t, _) = hhmm(secs);
            let (h, m): (u32, u32) = (t[0..2].parse().unwrap(), t[3..5].parse().unwrap());
            assert!(h < 24 && m < 60, "{secs} -> {t}");
        }
    }

    #[test]
    fn a_short_or_foreign_buffer_is_refused_rather_than_guessed() {
        assert_eq!(parse_tzif(b"", 0), None);
        assert_eq!(parse_tzif(b"NOTZ", 0), None);
        // Right magic, truncated body — must not read past the end.
        let mut d = b"TZif2".to_vec();
        d.resize(60, 0);
        assert_eq!(parse_tzif(&d, 0), None);
    }

    #[test]
    fn the_system_zone_is_a_whole_number_of_minutes() {
        // Not an assertion about WHICH zone — that differs per machine — but
        // about the shape of any answer we accept. Every real zone offset is a
        // multiple of 60 seconds; a parser that has drifted into the wrong
        // field of the ttinfo record almost always fails this.
        if let Offset::Resolved(o) = offset(1_755_000_000) {
            assert_eq!(o % 60, 0, "offset {o} is not a whole minute");
            assert!(o.abs() <= 14 * 3600, "offset {o} is outside UTC-12..UTC+14");
        }
    }
}
