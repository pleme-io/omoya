//! **mekuri (めくり) — the page-turn decision.**
//!
//! A frame is owed, or it is not. This crate owns that one decision and
//! nothing else, because it is the decision two independent pleme-io
//! renderers each got wrong — in opposite directions, from the same root.
//!
//! # Why this exists (measured 2026-08-21, plo)
//!
//! `mado` (GPU terminal) computed the verdict, recorded it, logged it, and
//! then rendered anyway:
//!
//! ```text
//! if peek_seqno == self.last_seqno && !blink_flip && !bell_active {
//!     TOTAL_FRAMES_SKIPPED.fetch_add(1, Ordering::Relaxed);
//!     tracing::debug!(path = "idle_peek_full", "idle frame …");
//!     // Fall through to full render below.
//! }
//! ```
//!
//! Its counter read `9_934_969` "skipped" of `10_726_562` — none of which
//! were skipped. Cost on a static screen: **50.7% of a core**, against a
//! source comment estimating "≈0.2% of one core … free correctness with no
//! measurable cost". A ~250x under-estimate, reasoned rather than measured.
//!
//! `omoya` (the compositor) made the same mistake with the operands
//! reversed: it composed a full frame first and *then* asked whether the
//! damage was empty, discarding the work when it was. **38.2% of a core
//! while presenting zero frames.**
//!
//! Neither was careless. Both had the decision as one statement and the
//! action as another, and nothing tied them together — so one drew what it
//! had decided to skip, and the other decided after it had already drawn.
//!
//! # The invariant
//!
//! **The decision PRODUCES the permission.** [`Gate::open`] returns a
//! [`Verdict`], and only its [`Verdict::Draw`] arm carries a [`Pass`].
//! Drawing and presenting require a `Pass`. There is no constructor for one
//! anywhere else, so:
//!
//! | illegal state | mechanism | tier |
//! |---|---|---|
//! | drawing work obtained from a `Skip` | [`Verdict::Skip`] carries no `Pass` | truly-unrep |
//! | a `Pass` conjured without deciding | only [`Gate::open`] constructs one | truly-unrep |
//! | two renderers draining one ledger | [`Gate`] is `!Clone`, `!Copy` | truly-unrep |
//! | verdict computed and ignored | [`Verdict`] is `#[must_use]` | only-mitigated (C6: a lint) |
//! | a frame's causes lost to an error path | [`Pass`] re-marks on drop | truly-unrep |
//! | consumer presents from its OWN path | [`Pass::spend`] is the ergonomic route | only-mitigated (C2) |
//!
//! **That last row is the honest ceiling, and it was found by red-running
//! this crate's own claim rather than by reasoning about it.** An earlier
//! draft of this table asserted flatly that "skipped, then presented" had no
//! code path. It compiled:
//!
//! ```ignore
//! match gate.open() {
//!     Verdict::Skip => present(),          // <- compiles fine
//!     Verdict::Draw(p) => p.presented(),
//! }
//! fn present() { /* takes no Pass, so nothing stops this */ }
//! ```
//!
//! mekuri cannot see a consumer's other functions, so it cannot make that
//! unrepresentable — only unattractive. [`Pass::spend`] exists to make the
//! correct shape the shortest one to write: the draw closure runs only from
//! inside a `Pass`, and its `Result` decides presented-vs-abandoned with no
//! separate statement to get wrong. Consumers should have exactly one
//! present path and it should take a `&Pass`.
//!
//! That last row is a bug neither original had noticed. When a render fails
//! partway, the reasons it was owed have already been drained; without
//! putting them back, the screen stays stale until something unrelated
//! happens to dirty it again. `Pass` restores its causes on
//! [`Pass::abandoned`] **and on an un-surrendered drop**, so the fail-safe
//! direction is one wasted frame rather than a frozen display.
//!
//! # What this crate deliberately does NOT own
//!
//! **Region damage.** `mado` tracks 1-D row spans; `omoya` tracks 2-D
//! rectangles. Same goal, genuinely different shapes — forcing them into one
//! type would be a bad abstraction that looks well-motivated. Each keeps its
//! own region tracking; mekuri answers only *whether* a frame is owed and
//! *why*.
//!
//! # Use
//!
//! ```
//! use mekuri::{Cause, Gate, Verdict};
//!
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! enum Why { Commit, Pointer, Clock }
//!
//! impl Cause for Why {
//!     fn bit(self) -> u8 {
//!         match self { Why::Commit => 0, Why::Pointer => 1, Why::Clock => 2 }
//!     }
//!     fn all() -> &'static [Self] { &[Why::Commit, Why::Pointer, Why::Clock] }
//! }
//!
//! let mut gate: Gate<Why> = Gate::new();
//! let ledger = gate.ledger();          // cheap, Clone + Send + Sync
//!
//! // Nothing has happened: the frame is not owed.
//! assert!(matches!(gate.open(), Verdict::Skip));
//!
//! // A client commits.
//! ledger.mark(Why::Commit);
//! match gate.open() {
//!     Verdict::Draw(pass) => {
//!         assert_eq!(pass.causes(), vec![Why::Commit]);
//!         // … compose, flip …
//!         pass.presented();
//!     }
//!     Verdict::Skip => unreachable!("a commit owes a frame"),
//! }
//!
//! // Drained exactly once.
//! assert!(matches!(gate.open(), Verdict::Skip));
//! ```

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// The maximum number of distinct causes a [`Gate`] can distinguish.
///
/// The ledger is a single `AtomicU64`, so marking is one atomic OR and
/// draining is one atomic swap — no lock, no allocation, on either path.
pub const MAX_CAUSES: u8 = 64;

/// A reason a frame is owed.
///
/// The set is **enumerable** ([`Cause::all`]) so a drained bitmask can be
/// rendered back into names — an operator asking "why did it redraw?" gets
/// an answer, not a number.
pub trait Cause: Copy + core::fmt::Debug + 'static {
    /// This cause's distinct bit, in `0..64`.
    ///
    /// # Panics
    /// Debug-asserted `< 64` at mark time. Two causes sharing a bit is a
    /// programming error this crate cannot detect for you — [`Cause::all`]
    /// makes it testable, and [`bits_are_distinct`] does the test.
    fn bit(self) -> u8;

    /// Every cause, so a mask can be decoded.
    fn all() -> &'static [Self]
    where
        Self: Sized;
}

/// Assert that a [`Cause`] impl assigns each variant a distinct bit.
///
/// Call this from a unit test. Two variants sharing a bit silently merges
/// two reasons into one, which is invisible until an operator is reading a
/// cause list that is missing an entry.
#[must_use]
pub fn bits_are_distinct<C: Cause>() -> bool {
    let mut seen: u64 = 0;
    for c in C::all() {
        let b = c.bit();
        if b >= MAX_CAUSES {
            return false;
        }
        let m = 1u64 << b;
        if seen & m != 0 {
            return false;
        }
        seen |= m;
    }
    true
}

#[derive(Debug, Default)]
struct Inner {
    bits: AtomicU64,
}

/// A handle that MARKS a frame as owed.
///
/// `Clone + Send + Sync` on purpose: the things that dirty a screen are
/// scattered (a surface commit, a pointer move, a clock tick, a resize) and
/// each should be able to say so without reaching the renderer.
///
/// Marking is one relaxed-ordering atomic OR. It is safe to call from any
/// thread, in any quantity — sixty marks between two frames cost the same as
/// one, which is the point.
#[derive(Debug, Clone)]
pub struct Ledger<C: Cause> {
    inner: Arc<Inner>,
    _cause: core::marker::PhantomData<fn() -> C>,
}

impl<C: Cause> Ledger<C> {
    /// Record that `cause` has made a frame owed.
    pub fn mark(&self, cause: C) {
        let b = cause.bit();
        debug_assert!(
            b < MAX_CAUSES,
            "Cause::bit() must be < 64; got {b}. See mekuri::MAX_CAUSES."
        );
        if b < MAX_CAUSES {
            self.inner.bits.fetch_or(1u64 << b, Ordering::Release);
        }
    }

    /// Whether a frame is currently owed, WITHOUT draining.
    ///
    /// **Diagnostic only.** Deciding on this and then drawing is exactly the
    /// decoupling this crate exists to prevent: the answer can change between
    /// the read and the draw, and nothing ties them together. Use it to
    /// publish a status leaf; use [`Gate::open`] to decide.
    #[must_use]
    pub fn peek_owed(&self) -> bool {
        self.inner.bits.load(Ordering::Acquire) != 0
    }
}

// SAFETY-adjacent note: `PhantomData<fn() -> C>` is covariant and imposes no
// auto-trait bound on `C`, so `Ledger` is `Send + Sync` because `Arc<Inner>`
// is — which is what we want, since `C` is only ever used as a bit index.

/// The single point at which the page-turn decision is made.
///
/// **Deliberately `!Clone` and `!Copy`.** A ledger drained in two places is
/// two renderers each seeing half the reasons, and each concluding the other
/// half did not happen. One screen, one gate.
#[derive(Debug)]
pub struct Gate<C: Cause> {
    ledger: Ledger<C>,
}

impl<C: Cause> Default for Gate<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Cause> Gate<C> {
    /// A gate with nothing owed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ledger: Ledger {
                inner: Arc::new(Inner::default()),
                _cause: core::marker::PhantomData,
            },
        }
    }

    /// A cheap, shareable handle for marking. Clone it freely.
    #[must_use]
    pub fn ledger(&self) -> Ledger<C> {
        self.ledger.clone()
    }

    /// Take the decision, draining every reason accumulated since the last
    /// call.
    ///
    /// Returns [`Verdict::Draw`] carrying the permission to render and
    /// present exactly one frame, or [`Verdict::Skip`] — which carries no
    /// such permission, so there is nothing to accidentally act on.
    #[must_use = "the verdict IS the decision — computing it and then drawing \
                  anyway is the exact defect mekuri exists to prevent"]
    pub fn open(&mut self) -> Verdict<C> {
        let mask = self.ledger.inner.bits.swap(0, Ordering::AcqRel);
        if mask == 0 {
            Verdict::Skip
        } else {
            Verdict::Draw(Pass {
                mask,
                inner: Arc::clone(&self.ledger.inner),
                surrendered: false,
                _cause: core::marker::PhantomData,
            })
        }
    }

    /// Mark through this gate directly, for a producer that happens to hold
    /// it. Equivalent to `self.ledger().mark(cause)`.
    pub fn mark(&self, cause: C) {
        self.ledger.mark(cause);
    }

    /// Whether a frame is owed, WITHOUT draining. Diagnostic only — see
    /// [`Ledger::peek_owed`].
    #[must_use]
    pub fn peek_owed(&self) -> bool {
        self.ledger.peek_owed()
    }
}

/// The outcome of [`Gate::open`].
///
/// `#[must_use]`: a verdict that is computed and dropped is a decision that
/// was made and then ignored, which is how `mado` came to render ten million
/// frames it had already ruled unnecessary.
#[must_use = "a verdict that is dropped is a decision that was ignored"]
#[derive(Debug)]
pub enum Verdict<C: Cause> {
    /// A frame is owed. The [`Pass`] is the permission to draw it.
    Draw(Pass<C>),
    /// Nothing has changed. **Do not draw, and do not present** — an
    /// unwritten swapchain slot handed to `present()` is where the
    /// shadow/afterimage class comes from. Skipping both is correct; the
    /// display keeps showing the last good frame on its own.
    Skip,
}

impl<C: Cause> Verdict<C> {
    /// Whether this verdict owes a frame.
    #[must_use]
    pub const fn is_draw(&self) -> bool {
        matches!(self, Verdict::Draw(_))
    }
}

/// Permission to render and present exactly one frame.
///
/// Non-`Clone` and non-`Copy`: the permission is for **one** frame, and a
/// copyable permission is not a permission.
///
/// Surrender it with [`Pass::presented`] when the frame reached the display,
/// or [`Pass::abandoned`] when it did not. Dropping it without either is
/// treated as abandonment — the causes go back on the ledger rather than
/// being lost, because a wasted frame is recoverable and a frozen screen is
/// not.
#[must_use = "a pass that is dropped without `presented()` or `abandoned()` \
              restores its causes and costs a redundant frame"]
#[derive(Debug)]
pub struct Pass<C: Cause> {
    mask: u64,
    inner: Arc<Inner>,
    surrendered: bool,
    _cause: core::marker::PhantomData<fn() -> C>,
}

impl<C: Cause> Pass<C> {
    /// Every reason this frame is owed, decoded from the drained mask.
    ///
    /// Order follows [`Cause::all`], so it is stable and testable rather than
    /// bit order.
    #[must_use]
    pub fn causes(&self) -> Vec<C> {
        C::all()
            .iter()
            .filter(|c| {
                let b = c.bit();
                b < MAX_CAUSES && self.mask & (1u64 << b) != 0
            })
            .copied()
            .collect()
    }

    /// Whether a specific cause contributed to this frame.
    #[must_use]
    pub fn caused_by(&self, cause: C) -> bool {
        let b = cause.bit();
        b < MAX_CAUSES && self.mask & (1u64 << b) != 0
    }

    /// The raw drained bitmask — for a status leaf that wants a number.
    #[must_use]
    pub const fn mask(&self) -> u64 {
        self.mask
    }

    /// Draw the frame, and surrender the pass according to the outcome.
    ///
    /// **The route consumers should take.** `draw` runs only from inside a
    /// held `Pass`, and its `Result` decides the surrender — `Ok` is
    /// [`presented`](Self::presented), `Err` is
    /// [`abandoned`](Self::abandoned), with the causes restored so the next
    /// tick retries. There is no separate "and now surrender it" statement to
    /// forget, which is the whole failure mode this crate was extracted to
    /// remove.
    ///
    /// ```
    /// # use mekuri::{Cause, Gate, Verdict};
    /// # #[derive(Clone, Copy, Debug, PartialEq, Eq)] enum W { Commit }
    /// # impl Cause for W {
    /// #   fn bit(self) -> u8 { 0 }
    /// #   fn all() -> &'static [Self] { &[W::Commit] }
    /// # }
    /// # let mut gate: Gate<W> = Gate::new();
    /// # gate.mark(W::Commit);
    /// if let Verdict::Draw(pass) = gate.open() {
    ///     let r: Result<(), &str> = pass.spend(|causes| {
    ///         assert_eq!(causes, [W::Commit]);
    ///         Ok(())          // composed and flipped
    ///     });
    ///     assert!(r.is_ok());
    /// }
    /// ```
    pub fn spend<T, E>(self, draw: impl FnOnce(&[C]) -> Result<T, E>) -> Result<T, E> {
        let causes = self.causes();
        match draw(&causes) {
            Ok(t) => {
                self.presented();
                Ok(t)
            }
            Err(e) => {
                self.abandoned();
                Err(e)
            }
        }
    }

    /// The frame reached the display. Consumes the permission.
    ///
    /// Prefer [`spend`](Self::spend), which cannot be forgotten.
    pub fn presented(mut self) {
        self.surrendered = true;
    }

    /// The frame did NOT reach the display — a render error, a refused flip,
    /// a lost device.
    ///
    /// **Restores the causes to the ledger**, so the next tick still knows a
    /// frame is owed. Without this, the reasons were already drained when the
    /// error happened and the screen would stay stale until something
    /// unrelated dirtied it.
    pub fn abandoned(mut self) {
        self.surrendered = true;
        self.inner.bits.fetch_or(self.mask, Ordering::Release);
    }
}

impl<C: Cause> Drop for Pass<C> {
    fn drop(&mut self) {
        if !self.surrendered {
            // Fail toward a redundant frame, never toward a frozen screen.
            self.inner.bits.fetch_or(self.mask, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Why {
        Commit,
        Pointer,
        Clock,
    }

    impl Cause for Why {
        fn bit(self) -> u8 {
            match self {
                Why::Commit => 0,
                Why::Pointer => 1,
                Why::Clock => 2,
            }
        }
        fn all() -> &'static [Self] {
            &[Why::Commit, Why::Pointer, Why::Clock]
        }
    }

    #[test]
    fn a_quiet_gate_owes_nothing() {
        let mut g: Gate<Why> = Gate::new();
        assert!(matches!(g.open(), Verdict::Skip));
        assert!(!g.peek_owed());
    }

    #[test]
    fn a_mark_owes_exactly_one_frame() {
        let mut g: Gate<Why> = Gate::new();
        g.ledger().mark(Why::Commit);
        let Verdict::Draw(p) = g.open() else {
            panic!("a commit owes a frame")
        };
        assert_eq!(p.causes(), alloc::vec![Why::Commit]);
        p.presented();
        assert!(matches!(g.open(), Verdict::Skip), "drained exactly once");
    }

    #[test]
    fn repeated_marks_between_frames_cost_one_frame() {
        let mut g: Gate<Why> = Gate::new();
        let l = g.ledger();
        for _ in 0..1000 {
            l.mark(Why::Commit);
        }
        let Verdict::Draw(p) = g.open() else {
            panic!()
        };
        assert_eq!(p.causes(), alloc::vec![Why::Commit]);
        p.presented();
        assert!(matches!(g.open(), Verdict::Skip));
    }

    #[test]
    fn distinct_causes_accumulate_and_are_all_reported() {
        let mut g: Gate<Why> = Gate::new();
        let l = g.ledger();
        l.mark(Why::Clock);
        l.mark(Why::Commit);
        let Verdict::Draw(p) = g.open() else {
            panic!()
        };
        // Order follows Cause::all(), not mark order — stable and testable.
        assert_eq!(p.causes(), alloc::vec![Why::Commit, Why::Clock]);
        assert!(p.caused_by(Why::Clock));
        assert!(!p.caused_by(Why::Pointer));
        p.presented();
    }

    #[test]
    fn an_abandoned_frame_stays_owed() {
        let mut g: Gate<Why> = Gate::new();
        g.ledger().mark(Why::Commit);
        let Verdict::Draw(p) = g.open() else {
            panic!()
        };
        p.abandoned();
        let Verdict::Draw(p2) = g.open() else {
            panic!("an abandoned frame is still owed — this is the frozen-screen guard")
        };
        assert_eq!(p2.causes(), alloc::vec![Why::Commit]);
        p2.presented();
    }

    #[test]
    fn a_dropped_pass_stays_owed() {
        let mut g: Gate<Why> = Gate::new();
        g.ledger().mark(Why::Pointer);
        {
            let Verdict::Draw(p) = g.open() else {
                panic!()
            };
            drop(p); // an early `?` return, a panic-free error path
        }
        assert!(
            g.peek_owed(),
            "dropping a pass must restore its causes, not lose the frame"
        );
    }

    #[test]
    fn marks_during_a_frame_owe_the_next_one() {
        // The race that matters: a client commits WHILE we are compositing.
        // Draining before the render means the new mark lands after the swap
        // and is owed to the following frame — never merged into the one
        // already in flight, and never dropped.
        let mut g: Gate<Why> = Gate::new();
        let l = g.ledger();
        l.mark(Why::Commit);
        let Verdict::Draw(p) = g.open() else {
            panic!()
        };
        l.mark(Why::Pointer); // arrives mid-composite
        p.presented();
        let Verdict::Draw(p2) = g.open() else {
            panic!("the mid-frame mark must owe the next frame")
        };
        assert_eq!(p2.causes(), alloc::vec![Why::Pointer]);
        p2.presented();
    }

    #[test]
    fn spend_presents_on_ok_and_drains() {
        let mut g: Gate<Why> = Gate::new();
        g.mark(Why::Commit);
        let Verdict::Draw(p) = g.open() else {
            panic!()
        };
        let out: Result<u8, ()> = p.spend(|causes| {
            assert_eq!(causes, [Why::Commit]);
            Ok(7)
        });
        assert_eq!(out, Ok(7));
        assert!(matches!(g.open(), Verdict::Skip), "an Ok frame is spent");
    }

    #[test]
    fn spend_restores_the_causes_on_err() {
        // The error path both originals got wrong: a failed render must not
        // consume the reason the frame was owed, or the screen stays stale.
        let mut g: Gate<Why> = Gate::new();
        g.mark(Why::Commit);
        let Verdict::Draw(p) = g.open() else {
            panic!()
        };
        let out: Result<(), &str> = p.spend(|_| Err("flip refused"));
        assert_eq!(out, Err("flip refused"));
        let Verdict::Draw(p2) = g.open() else {
            panic!("a failed frame is still owed")
        };
        assert_eq!(p2.causes(), alloc::vec![Why::Commit]);
        p2.presented();
    }

    #[test]
    fn cause_bits_are_distinct() {
        assert!(bits_are_distinct::<Why>());
    }

    #[test]
    fn a_colliding_cause_impl_is_caught() {
        #[derive(Clone, Copy, Debug)]
        enum Bad {
            A,
            B,
        }
        impl Cause for Bad {
            fn bit(self) -> u8 {
                0 // both variants — the silent merge this checks for
            }
            fn all() -> &'static [Self] {
                &[Bad::A, Bad::B]
            }
        }
        assert!(!bits_are_distinct::<Bad>());
    }

    #[test]
    fn an_out_of_range_bit_is_caught() {
        #[derive(Clone, Copy, Debug)]
        enum TooBig {
            X,
        }
        impl Cause for TooBig {
            fn bit(self) -> u8 {
                64
            }
            fn all() -> &'static [Self] {
                &[TooBig::X]
            }
        }
        assert!(!bits_are_distinct::<TooBig>());
    }

    #[test]
    fn peek_does_not_drain() {
        let mut g: Gate<Why> = Gate::new();
        g.ledger().mark(Why::Commit);
        assert!(g.peek_owed());
        assert!(g.peek_owed(), "peeking twice must not consume the reason");
        let Verdict::Draw(p) = g.open() else {
            panic!("peek must not have drained the ledger")
        };
        p.presented();
    }

    #[test]
    fn a_ledger_is_shareable_across_threads() {
        extern crate std;
        let mut g: Gate<Why> = Gate::new();
        let l = g.ledger();
        let h = std::thread::spawn(move || l.mark(Why::Pointer));
        h.join().unwrap();
        let Verdict::Draw(p) = g.open() else {
            panic!("a mark from another thread must owe a frame")
        };
        assert_eq!(p.causes(), alloc::vec![Why::Pointer]);
        p.presented();
    }
}
