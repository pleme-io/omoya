//! denpa (伝播) — damage DERIVED and baselined, not asserted.
//!
//! ── ★ THE DEFECT THIS EXISTS TO REMOVE ──────────────────────────────────────
//!
//! A damage region is only meaningful as a PAIR: `(region, baseline)` — "these
//! pixels differ *from that content*". Every protocol and every counter in this
//! stack carries the region and drops the baseline, and every stale-pixel bug
//! chased on 2026-08-30 lived in that gap.
//!
//! The consequence is sharper than it sounds. When the baseline is missing,
//! "nothing changed since a revision I can no longer account for" and "nothing
//! changed" render as the SAME empty answer. A consumer cannot tell them apart,
//! takes the cheap path, and the pixels it skipped stay stale forever.
//!
//! ── ★ WHAT IS SEALED, AND THE PART THAT IS NOT ──────────────────────────────
//!
//! Sealed: [`DamageLedger::since`] returns `Err(StaleBaseline)` — never an empty
//! `Vec` — when asked about a revision older than it can account for. The
//! caller cannot accidentally read "no damage" out of "I do not know", because
//! the two are different types.
//!
//! NOT sealed: that every mutation records itself. This is the demand-driven
//! half of the design (Adapton's demanded-computation graph, Salsa's revisions)
//! reduced to its one load-bearing primitive; it does not yet TRACK reads, so a
//! caller that mutates without calling [`DamageLedger::record`] still produces
//! undamaged changes. Closing that needs the reactive graph proper, where a read
//! cannot go unrecorded because performing it *is* the record. Tier:
//! only-mitigated (C1 — a convention that record() is called), and the ceiling
//! is named rather than implied.

use std::collections::VecDeque;

// ── ★ Revision: RETIRED IN PLACE, NOT DELETED ───────────────────────────────
//
// This module used to declare its own `pub struct Revision(u64)` and SAID SO in
// the doc comment: "deliberately the same shape as `mekuri::kentou::Revision`".
// A knowing re-spelling of an imported type is a copy, not a convergence —
// `mekuri = "0.1.2"` has been in this crate's `Cargo.toml` the whole time, and
// `nuri_renderer.rs` already calls `mekuri::kentou::Revision::ORIGIN`.
//
// ★ This is a RE-EXPORT, not a deletion (MODULARIZE, DON'T DELETE).
// `denpa::Revision` still resolves, to the same type mekuri hands the renderer,
// so the ledger and the scanout path now compare the SAME revision rather than
// two that merely look alike.
//
// What mekuri's adds that the local copy lacked: `Hash`, a saturating `next()`
// (the local one had no advance method at all), and `from_raw` — which is how
// the ledger builds one now that the tuple field belongs to another crate.
//
// ★ `Coverage` is deliberately NOT re-exported here. See `StaleBaseline` below.
pub use mekuri::kentou::Revision;

// ── ★ THE GUARD ─────────────────────────────────────────────────────────────
//
// This item only compiles while `Revision` IS mekuri's type. Re-introducing a
// local `struct Revision(u64)` — the exact regression this retirement exists to
// forbid — fails HERE with a type mismatch instead of silently forking the
// fleet's revision vocabulary a second time.
//
// `const _` rather than a `#[test]`: a test proves it at test time, a `const _`
// proves it at every build, including the release one.
const _: () = {
    #[allow(dead_code)]
    const fn revision_is_mekuris(r: mekuri::kentou::Revision) -> Revision {
        r
    }
};

// ── ★ WHY `StaleBaseline` DID NOT MOVE WITH `Revision` ──────────────────────
//
// `mekuri::kentou::Coverage::StaleBaseline` shares this one's NAME and neither
// its shape nor its question. Measured 2026-08-30, three independent reasons:
//
//  1. It is an enum VARIANT, not a type. `since` returns `Result<_, E>` and a
//     variant cannot be an `E` (`E0573: expected type, found variant`). Using
//     the whole `Coverage` widens the error with an `OutOfBounds` arm this
//     ledger can never produce — adding a bad state, not removing one.
//  2. The fields are a different fact. mekuri's are
//     `{ damage_base, target }: u64` — "the damage's base is not the revision
//     this target holds", an IDENTITY mismatch. Ours are
//     `{ asked, oldest_retained }: Revision` — "history no longer reaches back
//     that far", a RETENTION fact. Neither field name means the other.
//  3. The `Display` text differs, and this module's load-bearing test asserts
//     on it (`contains("full repaint")`). `Coverage`'s says "the frames between
//     are unaccounted for" and never says "full repaint".
//
// Same goal, different shapes ⇒ write the rule down, do not force one type.

/// The ledger cannot account for the requested baseline.
///
/// ★ THIS IS THE WHOLE POINT OF THE MODULE. It is a distinct type from "no
/// damage" so that a consumer physically cannot treat one as the other. The
/// honest response is a full repaint — expensive and correct — and the caller
/// is forced to write that down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleBaseline {
    /// What was asked for.
    pub asked: Revision,
    /// The oldest revision still accounted for.
    pub oldest_retained: Revision,
}

impl core::fmt::Display for StaleBaseline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "damage asked from revision {} but the ledger only accounts back to \
             {} — the honest answer is a full repaint, not an empty region",
            self.asked.get(),
            self.oldest_retained.get()
        )
    }
}

impl core::error::Error for StaleBaseline {}

/// A bounded history of what changed, and when.
#[derive(Debug)]
pub struct DamageLedger {
    current: u64,
    /// `(revision_at_which_it_changed, rect)`, oldest first.
    history: VecDeque<(u64, (i32, i32, i32, i32))>,
    capacity: usize,
}

impl DamageLedger {
    /// A ledger retaining at most `capacity` entries.
    ///
    /// Capacity is finite on purpose: unbounded history is a leak, and a ledger
    /// that quietly forgets is exactly the failure this module refuses. Finite
    /// plus an honest `Err` beats infinite plus an OOM.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            current: 0,
            history: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// The current revision.
    #[must_use]
    pub const fn revision(&self) -> Revision {
        Revision::from_raw(self.current)
    }

    /// The oldest revision this ledger can still answer for.
    #[must_use]
    pub fn oldest_retained(&self) -> Revision {
        self.history
            .front()
            .map_or(Revision::from_raw(self.current), |&(r, _)| {
                Revision::from_raw(r - 1)
            })
    }

    /// Record a change, advancing the revision.
    pub fn record(&mut self, rect: (i32, i32, i32, i32)) -> Revision {
        self.current += 1;
        self.history.push_back((self.current, rect));
        while self.history.len() > self.capacity {
            self.history.pop_front();
        }
        Revision::from_raw(self.current)
    }

    /// What changed since `base`.
    ///
    /// # Errors
    /// [`StaleBaseline`] when `base` predates the retained history — NOT an
    /// empty result, which is the distinction the whole module exists for.
    pub fn since(&self, base: Revision) -> Result<Vec<(i32, i32, i32, i32)>, StaleBaseline> {
        if base.get() > self.current {
            // A baseline from the future is nonsense; refuse rather than
            // silently return nothing, which would read as "up to date".
            return Err(StaleBaseline {
                asked: base,
                oldest_retained: self.oldest_retained(),
            });
        }
        let oldest = self.oldest_retained();
        if base < oldest {
            return Err(StaleBaseline {
                asked: base,
                oldest_retained: oldest,
            });
        }
        Ok(self
            .history
            .iter()
            .filter(|&&(r, _)| r > base.get())
            .map(|&(_, rect)| rect)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: (i32, i32, i32, i32) = (0, 0, 10, 10);
    const B: (i32, i32, i32, i32) = (10, 10, 5, 5);

    #[test]
    fn nothing_recorded_means_nothing_changed_and_that_is_not_an_error() {
        let l = DamageLedger::new(8);
        assert_eq!(l.since(Revision::ORIGIN), Ok(vec![]));
    }

    #[test]
    fn damage_since_an_older_revision_includes_everything_after_it() {
        let mut l = DamageLedger::new(8);
        let r1 = l.record(A);
        l.record(B);
        assert_eq!(l.since(r1), Ok(vec![B]));
        assert_eq!(l.since(Revision::ORIGIN), Ok(vec![A, B]));
        assert_eq!(l.since(l.revision()), Ok(vec![]));
    }

    /// ★ THE LOAD-BEARING TEST. A baseline the ledger cannot account for must
    /// come back as an ERROR, never as an empty region — the two are
    /// indistinguishable to a caller that only has a `Vec`, and that
    /// indistinguishability is the stale-pixel class.
    #[test]
    fn a_baseline_older_than_history_errors_rather_than_reading_as_no_damage() {
        let mut l = DamageLedger::new(2);
        let ancient = l.record(A);
        l.record(B);
        l.record(A);
        l.record(B); // `ancient` is now evicted
        let err = l
            .since(ancient)
            .expect_err("an unaccountable baseline must not answer 'nothing changed'");
        assert_eq!(err.asked, ancient);
        assert!(err.to_string().contains("full repaint"));
    }

    /// ANTI-VACUITY for the test above: with enough capacity the SAME query
    /// succeeds, so the error is a property of the eviction and not of the
    /// query always failing.
    #[test]
    fn the_same_query_succeeds_when_history_is_retained() {
        let mut l = DamageLedger::new(64);
        let ancient = l.record(A);
        l.record(B);
        l.record(A);
        l.record(B);
        assert_eq!(l.since(ancient), Ok(vec![B, A, B]));
    }

    #[test]
    fn capacity_is_bounded_so_history_cannot_leak() {
        let mut l = DamageLedger::new(4);
        for _ in 0..1000 {
            l.record(A);
        }
        assert!(l.history.len() <= 4);
        assert_eq!(l.revision().get(), 1000);
    }

    /// A revision from the future is refused, not answered with silence.
    #[test]
    fn a_future_baseline_is_refused() {
        let l = DamageLedger::new(8);
        assert!(l.since(Revision::from_raw(99)).is_err());
    }

    #[test]
    fn a_zero_capacity_ledger_still_retains_one_entry() {
        let mut l = DamageLedger::new(0);
        let r = l.record(A);
        assert_eq!(l.since(Revision::from_raw(r.get() - 1)), Ok(vec![A]));
    }
}
