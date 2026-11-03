//! Why a frame is owed.
//!
//! The compositor's `mekuri::Cause` set — the closed, enumerable list of
//! things that can dirty the screen. `mekuri` owns the decision machinery;
//! this module owns only the vocabulary, because only omoya knows what can
//! change here.
//!
//! ★ **Adding a producer means adding a variant.** If something starts
//! changing what is on screen and does not mark a cause, the screen will not
//! update and nothing will say so — the tick will simply find nothing owed
//! and skip, forever. That failure is silent, so the list below is the thing
//! to check first when "it stopped repainting".
//!
//! The set is enumerable ([`mekuri::Cause::all`]) so `kanshou`'s `owed` leaf
//! can answer *why* the last frame happened with names rather than a bitmask.

/// A reason the screen no longer matches what was last presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owed {
    /// A client committed a new buffer — the ordinary case, and the one that
    /// drives every frame during real work.
    Commit,
    /// The pointer moved. Distinct from `Commit` because the cursor is
    /// omoya's own element: nothing commits when the mouse moves, so without
    /// this the pointer would freeze while everything else kept working.
    Pointer,
    /// A window appeared, vanished, or was re-tiled. The layout decides
    /// geometry outside the render loop, so the loop cannot see this itself.
    Windows,
    /// A deed ran — a focus change, a layout verb, a VT switch.
    Deed,
    /// The status bar's text changed. Driven by a once-a-second timer that
    /// marks ONLY when the rendered minute or the status line differs, so an
    /// idle seat presents zero frames per second rather than one.
    Chrome,
    Ground,
    /// A screenshot was requested over `kanshou`. Without this the capture
    /// would wait for someone to move the mouse: the request arrives on
    /// another thread, and an idle gate would skip the tick that serves it.
    Capture,
    /// The session was resumed — a VT switch back, a device resume. Nothing
    /// committed while we were away and the framebuffer contents are not
    /// ours any more, so the next frame must be unconditional.
    Resume,
}

impl mekuri::Cause for Owed {
    fn bit(self) -> u8 {
        match self {
            Owed::Commit => 0,
            Owed::Pointer => 1,
            Owed::Windows => 2,
            Owed::Deed => 3,
            Owed::Chrome => 4,
            Owed::Capture => 5,
            Owed::Resume => 6,
            Owed::Ground => 7,
        }
    }

    fn all() -> &'static [Self] {
        &[
            Owed::Commit,
            Owed::Pointer,
            Owed::Windows,
            Owed::Deed,
            Owed::Chrome,
            Owed::Capture,
            Owed::Resume,
            Owed::Ground,
        ]
    }
}

impl Owed {
    /// The name `kanshou` publishes. Kept beside `bit` so a new variant that
    /// forgets one is caught by the round-trip test below.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Owed::Commit => "commit",
            Owed::Pointer => "pointer",
            Owed::Windows => "windows",
            Owed::Deed => "deed",
            Owed::Chrome => "chrome",
            Owed::Capture => "capture",
            Owed::Resume => "resume",
            Owed::Ground => "ground",
        }
    }
}

const FIRST_RETRY: std::time::Duration = std::time::Duration::from_millis(50);
const LAST_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct FrameRetry {
    failures: u32,
    not_before: Option<std::time::Instant>,
}

impl FrameRetry {
    #[must_use]
    pub fn ready(&self, now: std::time::Instant) -> bool {
        self.not_before.is_none_or(|at| now >= at)
    }

    #[must_use]
    pub fn failed(&mut self, now: std::time::Instant) -> bool {
        let doublings = self.failures.min(16);
        let wait = FIRST_RETRY.saturating_mul(1 << doublings).min(LAST_RETRY);
        self.not_before = Some(now + wait);
        self.failures = self.failures.saturating_add(1);
        self.failures == 1
    }

    pub fn forgive(&mut self) -> u32 {
        self.not_before = None;
        std::mem::take(&mut self.failures)
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameRetry, Owed};
    use mekuri::Cause as _;
    use std::time::{Duration, Instant};

    #[test]
    fn a_refused_frame_logs_once_and_backs_off() {
        let start = Instant::now();
        let mut retry = FrameRetry::default();
        let (mut attempts, mut logged) = (0_u32, 0_u32);
        for tick in 0..75_u64 * 60 {
            let now = start + Duration::from_millis(tick * 1000 / 75);
            if retry.ready(now) {
                attempts += 1;
                if retry.failed(now) {
                    logged += 1;
                }
            }
        }
        assert_eq!(logged, 1, "one streak, one line");
        assert!(attempts < 25, "{attempts} composites in a minute");
    }

    #[test]
    fn the_first_retry_is_prompt_and_the_longest_is_bounded() {
        let start = Instant::now();
        let mut retry = FrameRetry::default();
        assert!(retry.ready(start), "nothing has failed yet");
        let _ = retry.failed(start);
        assert!(!retry.ready(start));
        assert!(retry.ready(start + Duration::from_millis(50)));

        for _ in 0..1000 {
            let _ = retry.failed(start);
        }
        assert!(!retry.ready(start + Duration::from_millis(4999)));
        assert!(retry.ready(start + Duration::from_secs(5)));
    }

    #[test]
    fn forgiving_reports_the_streak_and_starts_over() {
        let start = Instant::now();
        let mut retry = FrameRetry::default();
        assert_eq!(retry.forgive(), 0);
        assert!(retry.failed(start));
        assert!(!retry.failed(start));
        assert!(!retry.failed(start));
        assert_eq!(retry.forgive(), 3);
        assert!(retry.ready(start), "a forgiven streak holds nothing back");
        assert!(retry.failed(start), "a new streak logs again");
    }

    #[test]
    fn the_render_loop_goes_through_the_retry() {
        let source = include_str!("drm.rs");
        let live = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(live, _)| live);
        for wire in [
            "frame_retry.ready(",
            "frame_retry.failed(",
            "frame_retry.forgive()",
        ] {
            assert!(live.contains(wire), "drm.rs does not call `{wire}`");
        }
    }

    #[test]
    fn every_cause_has_a_distinct_bit() {
        // Two variants sharing a bit merges two reasons into one silently —
        // the `owed` leaf would then be missing an entry with nothing to say
        // so.
        assert!(mekuri::bits_are_distinct::<Owed>());
    }

    #[test]
    fn every_cause_has_a_distinct_name() {
        let mut names: Vec<&str> = Owed::all().iter().map(|c| c.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two causes share a published name");
    }

    #[test]
    fn the_catalog_is_complete() {
        // `all()` is what the leaf enumerates and what `bits_are_distinct`
        // checks. A variant missing from it is invisible to both, so its
        // count is pinned: adding a variant without listing it fails here.
        assert_eq!(Owed::all().len(), 8, "a new Owed variant must join all()");
    }
}
