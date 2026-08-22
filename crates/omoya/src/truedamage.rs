//! What actually changed — the compositor computing its own damage, because
//! the client cannot tell it.
//!
//! ── ★ THE MEASUREMENT THIS EXISTS FOR ─────────────────────────────────────
//! Taken on plo 2026-08-21 through kanshou, on the live seat:
//!
//! | cause | median frame |
//! |---|---|
//! | pointer-only motion | **60 µs** |
//! | one-character text commit | **4,207 µs** |
//!
//! The display runs 1920x1080@360, so one vblank interval is **2,778 µs**. A
//! keystroke frame cost **1.5 intervals** — it missed its flip *by
//! construction*, on every character, not occasionally.
//!
//! Splitting that frame:
//!
//! | stage | cost | share |
//! |---|---|---|
//! | gather (the shm import lives here) | 938 µs | 22.3% |
//! | `render_output` → `nuri::blit` | **3,270 µs** | **77.7%** |
//!
//! Both stages move the same ~16 MB (an 8 MB read and an 8 MB write over a
//! 1912×1044 surface). The import does it at ~17 GB/s and the blit at
//! ~4.9 GB/s. Same read, so **the write into the mapped scanout buffer is
//! ~3.5× more expensive than a write into ordinary RAM** — which is what a
//! DRM dumb buffer's mapping is: uncached or write-combining memory, where the
//! CPU's usual tricks do not apply.
//!
//! So the expensive thing is not the copying. It is **how many bytes reach
//! scanout memory**, and that number is set by the damage rectangle.
//!
//! ── ★ WHY THE CLIENT CANNOT FIX THIS, AND WE MUST ─────────────────────────
//! The damage is a lie, and mado's own source says so
//! (`mado/src/grid_damage.rs`): it computes a perfectly good per-row damage
//! set for its own draw, and then cannot tell us — presentation goes through
//! wgpu, and `SurfaceTexture::present()` takes no damage argument at all
//! (there is no `VK_KHR_incremental_present` equivalent exposed). So the whole
//! surface is reported dirty on every present no matter what the client knows.
//!
//! It is not mado's failing. It is true of every wgpu client, which on this
//! seat is every graphical client we ship.
//!
//! **So the compositor measures it.** We are already about to read the
//! client's buffer; comparing it against what that surface last committed
//! costs one more read of ordinary RAM (~470 µs) and can remove almost all of
//! a 3,270 µs write into slow memory. For a keystroke the truth is a handful
//! of text rows, not 1,044.
//!
//! ── ★ WHY IT HAPPENS AT COMMIT, NOT AT BLIT ───────────────────────────────
//! The obvious place is the blit — skip a row whose pixels did not change.
//! **That is wrong and it would leave stale pixels on screen.** The blit's
//! damage is not "what the client changed": it is what the client changed
//! UNION what is stale in *this* back buffer, which smithay derives from
//! `back_buffer_age()`. A row can be unchanged since the last commit and still
//! be wrong in the buffer we are about to scan out, because that buffer was
//! last drawn two frames ago.
//!
//! Shrinking `SurfaceAttributes::damage` *before*
//! `on_commit_buffer_handler` consumes it puts the refined truth in at the
//! bottom, and every union above it — age, elements, output — stays correct by
//! construction rather than by our care. Over-damaging is always safe;
//! under-damaging is the bug that reaches a screen. This module can only ever
//! *shrink* what the client declared, and it refuses to shrink at all whenever
//! it cannot prove the comparison is meaningful.

use std::collections::HashMap;

/// A half-open run of buffer rows `[start, end)` that differ.
///
/// Rows rather than rectangles, deliberately. A per-pixel or per-column diff
/// would find smaller regions and cost far more to compute, and the shape that
/// matters here is a text row: the thing a keystroke changes is one line of
/// cells, which is a band of ~20 pixel rows spanning the full width. Chasing
/// columns would spend the read budget we are trying to save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSpan {
    pub start: usize,
    pub end: usize,
}

/// The verdict, with the denominator inside it.
///
/// ★ A bare `Vec<RowSpan>` would make "found nothing" and "did not look"
/// identical, and those are opposite facts: the first means the frame is free,
/// the second means the refinement is broken and every frame is silently
/// paying full price. `Refused` names which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The comparison ran. `spans` is what actually differs; `rows_examined`
    /// is what it looked at, so a caller can report a ratio rather than a
    /// count.
    Refined {
        spans: Vec<RowSpan>,
        rows_examined: usize,
    },
    /// No comparison was possible — first commit for this surface, a geometry
    /// or format change, or a buffer we cannot address. The client's declared
    /// damage stands **unmodified**.
    Refused(&'static str),
}

impl Verdict {
    /// Rows this verdict says must be redrawn, or `None` when it refused.
    #[must_use]
    pub fn rows(&self) -> Option<usize> {
        match self {
            Self::Refined { spans, .. } => Some(spans.iter().map(|s| s.end - s.start).sum()),
            Self::Refused(_) => None,
        }
    }
}

/// Compare two same-shaped buffers and return the rows that differ.
///
/// Pure — no Wayland, no compositor, no allocation beyond the result — so the
/// part of this module that can be wrong in an interesting way is testable
/// with two `Vec<u8>`s and nothing else.
///
/// ★ **Row equality is `==` on slices, which lowers to `memcmp`.** That is the
/// whole reason this is affordable: `memcmp` bails at the first differing byte,
/// so an unchanged row costs a full-width compare and a changed row usually
/// costs a few bytes. Both operands are ordinary RAM.
///
/// Refuses rather than guesses when the two do not describe the same picture.
/// A stride or height mismatch means the surface was resized or reformatted,
/// and comparing across that boundary would report garbage rows as unchanged —
/// under-damage, the one direction that breaks.
#[must_use]
pub fn changed_rows(prev: &[u8], next: &[u8], stride: usize, height: usize) -> Verdict {
    if stride == 0 || height == 0 {
        return Verdict::Refused("empty geometry");
    }
    let needed = match stride.checked_mul(height) {
        Some(n) => n,
        None => return Verdict::Refused("stride * height overflows"),
    };
    if prev.len() < needed || next.len() < needed {
        return Verdict::Refused("buffer shorter than its geometry");
    }

    let mut spans: Vec<RowSpan> = Vec::new();
    let mut run: Option<usize> = None;
    for y in 0..height {
        let a = y * stride;
        let b = a + stride;
        let differs = prev[a..b] != next[a..b];
        match (differs, run) {
            // A changed row opens a run, or extends the one already open.
            (true, None) => run = Some(y),
            (true, Some(_)) => {}
            // An unchanged row closes it. Coalescing here rather than
            // afterwards keeps the result one span per contiguous band, which
            // is what a text edit actually produces.
            (false, Some(start)) => {
                spans.push(RowSpan { start, end: y });
                run = None;
            }
            (false, None) => {}
        }
    }
    if let Some(start) = run {
        spans.push(RowSpan {
            start,
            end: height,
        });
    }
    Verdict::Refined {
        spans,
        rows_examined: height,
    }
}

/// One surface's last committed pixels.
///
/// ★ Keyed by SURFACE, not by `wl_buffer`. A client cycles through a small
/// swapchain, so a buffer-keyed shadow would compare against that buffer's
/// previous use — two or three frames ago — and report the union of everything
/// that changed since. That is still a safe *superset*, but it is a
/// meaningfully worse one on exactly the workload this exists for: a cursor
/// blink two frames back would re-damage a band nothing has touched since.
#[derive(Debug)]
struct Shadow {
    data: Vec<u8>,
    stride: usize,
    height: usize,
    /// The fourcc the pixels were in. A format change makes the byte
    /// comparison meaningless even at identical dimensions.
    format: u32,
}

/// Per-surface shadows, and the counters that say whether refinement is
/// actually happening.
#[derive(Debug, Default)]
pub struct Shadows {
    by_surface: HashMap<u32, Shadow>,
    /// Commits where the comparison ran.
    pub refined: u64,
    /// Commits where it refused, and therefore paid full price.
    pub refused: u64,
    /// Rows the comparison said were dirty, summed.
    pub rows_dirty: u64,
    /// Rows it examined, summed. **The denominator** — `rows_dirty` alone
    /// cannot distinguish "nothing changes" from "nothing is being compared".
    pub rows_examined: u64,
}

impl Shadows {
    /// Compare `next` against this surface's last committed pixels, then adopt
    /// `next` as the new shadow.
    ///
    /// Returns the verdict. The shadow is updated **whether or not** the
    /// comparison succeeded, so the commit after a refusal can refine again —
    /// a resize should cost one full frame, not permanently disable this.
    pub fn refine(&mut self, key: u32, next: &[u8], stride: usize, height: usize, format: u32) -> Verdict {
        let verdict = match self.by_surface.get(&key) {
            None => Verdict::Refused("first commit for this surface"),
            Some(prev) if prev.stride != stride => Verdict::Refused("stride changed"),
            Some(prev) if prev.height != height => Verdict::Refused("height changed"),
            Some(prev) if prev.format != format => Verdict::Refused("format changed"),
            Some(prev) => changed_rows(&prev.data, next, stride, height),
        };

        // ★ Adopt unconditionally, and copy only what differs when we can.
        // On the refined path the untouched rows are already identical by
        // definition, so re-copying them would reintroduce the full-surface
        // write this module exists to remove — into RAM rather than scanout
        // memory, but 8 MB is 8 MB.
        match (&verdict, self.by_surface.get_mut(&key)) {
            (Verdict::Refined { spans, .. }, Some(shadow)) => {
                for s in spans {
                    let a = s.start * stride;
                    let b = s.end * stride;
                    if b <= shadow.data.len() && b <= next.len() {
                        shadow.data[a..b].copy_from_slice(&next[a..b]);
                    }
                }
            }
            _ => {
                self.by_surface.insert(
                    key,
                    Shadow {
                        data: next[..stride.saturating_mul(height).min(next.len())].to_vec(),
                        stride,
                        height,
                        format,
                    },
                );
            }
        }

        match &verdict {
            Verdict::Refined {
                spans,
                rows_examined,
            } => {
                self.refined += 1;
                self.rows_dirty += spans.iter().map(|s| (s.end - s.start) as u64).sum::<u64>();
                self.rows_examined += *rows_examined as u64;
            }
            Verdict::Refused(_) => self.refused += 1,
        }
        verdict
    }

    /// Drop a surface's shadow. Called when the surface is destroyed — a
    /// shadow outliving its surface is 8 MB of leak per window.
    pub fn forget(&mut self, key: u32) {
        self.by_surface.remove(&key);
    }

    /// How many shadows are held. Exposed so a leak is observable rather than
    /// inferred from RSS.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_surface.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_surface.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(rows: &[u8], stride: usize) -> Vec<u8> {
        rows.iter().flat_map(|v| std::iter::repeat(*v).take(stride)).collect()
    }

    #[test]
    fn the_wire_encoding_round_trips_every_mode() {
        // ★ The atomic is the SINGLE source of truth for the live knob, so a
        // lossy encoding would flip the seat into a mode nobody selected.
        for m in [Mode::Off, Mode::On, Mode::Verify] {
            assert_eq!(Mode::from_u64(m.to_u64()), m, "{m:?} must round-trip");
        }
        // And an out-of-range value is ON, never a silent Off.
        assert_eq!(Mode::from_u64(99), Mode::On);
    }

    #[test]
    fn an_unknown_mode_name_is_refused_with_the_accepted_set() {
        assert_eq!(Mode::parse("on"), Ok(Mode::On));
        assert_eq!(Mode::parse("verify"), Ok(Mode::Verify));
        assert_eq!(Mode::parse("off"), Ok(Mode::Off));
        let e = Mode::parse("ON").unwrap_err();
        assert!(e.contains("off, on, verify"), "the refusal must name the set: {e}");
    }

    #[test]
    fn only_on_is_allowed_to_replace_the_declaration() {
        // ★ The whole point of `Verify`: it pays for the measurement and
        // changes nothing on screen. If this ever returned true, the A/B would
        // be comparing a mode against itself and would exonerate a real bug.
        assert!(Mode::On.replaces_declaration());
        assert!(!Mode::Verify.replaces_declaration());
        assert!(!Mode::Off.replaces_declaration());
    }

    #[test]
    fn verify_computes_but_off_does_not() {
        // `Verify` must still run the comparison or its counters are fiction.
        assert!(Mode::Verify.computes());
        assert!(Mode::On.computes());
        assert!(!Mode::Off.computes());
    }

    #[test]
    fn an_identical_buffer_has_no_damage() {
        // ★ THE WHOLE POINT. mado re-renders and re-commits its entire surface
        // for a cursor blink; if the pixels did not change, nothing should
        // reach scanout memory at all.
        let a = buf(&[1, 2, 3, 4], 16);
        let v = changed_rows(&a, &a, 16, 4);
        assert_eq!(v.rows(), Some(0), "identical buffers must produce zero rows");
    }

    #[test]
    fn one_changed_row_yields_one_row() {
        let a = buf(&[1, 2, 3, 4], 16);
        let b = buf(&[1, 2, 9, 4], 16);
        let v = changed_rows(&a, &b, 16, 4);
        assert_eq!(
            v,
            Verdict::Refined {
                spans: vec![RowSpan { start: 2, end: 3 }],
                rows_examined: 4
            }
        );
    }

    #[test]
    fn adjacent_changed_rows_coalesce_into_one_span() {
        // A text edit dirties a BAND — one line of cells is ~20 pixel rows.
        // Emitting 20 single-row spans would multiply the per-rect overhead in
        // every consumer above this for no benefit.
        let a = buf(&[1, 1, 1, 1, 1, 1], 8);
        let b = buf(&[1, 9, 9, 9, 1, 1], 8);
        let v = changed_rows(&a, &b, 8, 6);
        assert_eq!(
            v,
            Verdict::Refined {
                spans: vec![RowSpan { start: 1, end: 4 }],
                rows_examined: 6
            }
        );
    }

    #[test]
    fn a_run_touching_the_last_row_is_closed() {
        // ★ The off-by-one that would silently drop the bottom of the screen.
        // The loop closes a run when it meets an UNCHANGED row; a run that
        // reaches the final row never meets one.
        let a = buf(&[1, 1, 1, 1], 8);
        let b = buf(&[1, 1, 9, 9], 8);
        let v = changed_rows(&a, &b, 8, 4);
        assert_eq!(v.rows(), Some(2), "a run at the bottom edge must be emitted");
        if let Verdict::Refined { spans, .. } = v {
            assert_eq!(spans, vec![RowSpan { start: 2, end: 4 }]);
        }
    }

    #[test]
    fn every_row_changed_reports_every_row() {
        // The honest worst case — video, a scroll — must still work and must
        // report the full height, so the ratio a caller publishes is truthful
        // rather than flattering.
        let a = buf(&[1, 1, 1, 1], 8);
        let b = buf(&[2, 2, 2, 2], 8);
        let v = changed_rows(&a, &b, 8, 4);
        assert_eq!(v.rows(), Some(4));
    }

    #[test]
    fn a_mismatched_shape_is_REFUSED_not_guessed() {
        // ★ THE DIRECTION THAT BREAKS. Under-damage leaves stale pixels on a
        // screen; over-damage only costs time. So every case this cannot prove
        // must refuse and let the client's own damage stand.
        let a = buf(&[1, 1], 8);
        assert!(matches!(changed_rows(&a, &a, 8, 99), Verdict::Refused(_)));
        assert!(matches!(changed_rows(&a, &a, 0, 2), Verdict::Refused(_)));
        assert!(matches!(changed_rows(&a, &a, 8, 0), Verdict::Refused(_)));
        assert!(matches!(changed_rows(&[], &[], 8, 2), Verdict::Refused(_)));
        assert_eq!(changed_rows(&a, &a, 8, 99).rows(), None);
    }

    #[test]
    fn the_first_commit_refuses_and_the_second_refines() {
        // A shadow has to exist before it can be compared against, and the
        // commit that creates it must not claim a saving it did not make.
        let mut sh = Shadows::default();
        let a = buf(&[1, 1, 1, 1], 8);
        assert!(matches!(sh.refine(7, &a, 8, 4, 0), Verdict::Refused(_)));
        assert_eq!(sh.refined, 0);
        assert_eq!(sh.refused, 1);

        let b = buf(&[1, 9, 1, 1], 8);
        assert_eq!(sh.refine(7, &b, 8, 4, 0).rows(), Some(1));
        assert_eq!(sh.refined, 1);
    }

    #[test]
    fn a_refusal_does_not_disable_refinement_forever() {
        // ★ A resize must cost ONE full frame, not permanently switch this
        // off. The shadow is adopted on the refusing commit too, so the next
        // one has something to compare against.
        let mut sh = Shadows::default();
        sh.refine(1, &buf(&[1, 1], 8), 8, 2, 0);
        // Geometry changes -> refuse, but adopt the new shape.
        assert!(matches!(
            sh.refine(1, &buf(&[1, 1, 1, 1], 8), 8, 4, 0),
            Verdict::Refused(_)
        ));
        // ...and the very next commit refines against it.
        let v = sh.refine(1, &buf(&[1, 1, 9, 1], 8), 8, 4, 0);
        assert_eq!(v.rows(), Some(1), "refinement must resume immediately");
    }

    #[test]
    fn the_shadow_tracks_the_latest_pixels_after_a_partial_update() {
        // The partial shadow update is the subtle one: only changed rows are
        // copied back, so a bug there makes the NEXT diff report a row as
        // dirty forever, or as clean when it is not.
        let mut sh = Shadows::default();
        sh.refine(1, &buf(&[1, 1, 1], 8), 8, 3, 0);
        sh.refine(1, &buf(&[1, 9, 1], 8), 8, 3, 0);
        // Committing the SAME pixels again must now be a no-op.
        let v = sh.refine(1, &buf(&[1, 9, 1], 8), 8, 3, 0);
        assert_eq!(v.rows(), Some(0), "the shadow did not adopt the changed row");
    }

    #[test]
    fn a_format_change_refuses_even_at_identical_dimensions() {
        // Same width, same height, different meaning per byte. A byte compare
        // across that boundary is not wrong-looking — it is confidently wrong.
        let mut sh = Shadows::default();
        let a = buf(&[1, 1], 8);
        sh.refine(1, &a, 8, 2, 0x3432_5258);
        assert!(matches!(
            sh.refine(1, &a, 8, 2, 0x3234_5241),
            Verdict::Refused(_)
        ));
    }

    #[test]
    fn forgetting_a_surface_releases_its_shadow() {
        // 8 MB per window; a shadow outliving its surface is a leak that only
        // shows up as RSS growth on a long-lived seat.
        let mut sh = Shadows::default();
        sh.refine(1, &buf(&[1], 8), 8, 1, 0);
        sh.refine(2, &buf(&[1], 8), 8, 1, 0);
        assert_eq!(sh.len(), 2);
        sh.forget(1);
        assert_eq!(sh.len(), 1);
        assert!(!sh.is_empty());
    }

    #[test]
    fn the_denominator_travels_with_the_count() {
        // ★ `rows_dirty` alone cannot distinguish "nothing is changing" from
        // "nothing is being compared", and those are opposite facts about the
        // health of this module. A caller that publishes one must publish both.
        let mut sh = Shadows::default();
        let a = buf(&[1, 1, 1, 1], 8);
        sh.refine(1, &a, 8, 4, 0);
        sh.refine(1, &a, 8, 4, 0);
        assert_eq!(sh.rows_dirty, 0, "nothing changed");
        assert_eq!(sh.rows_examined, 4, "but four rows WERE examined");
    }
}

// ── The Wayland half ─────────────────────────────────────────────────────
// Kept below the pure half and depending on it, never the other way round, so
// everything above stays testable with two `Vec<u8>`s and no compositor.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

/// How much authority the refinement is given over what reaches the screen.
///
/// ★ **THIS EXISTS BECAUSE THE SEAT IS THE OPERATOR'S ONLY LOCAL CONSOLE.**
/// The refinement is correct as far as 99 tests and a whole-stack VM run can
/// establish, and "as far as tests can establish" is not the same as "on the
/// machine you are sitting at". A change that can only be evaluated by
/// rebuilding and logging back in is a change nobody can A/B, and an
/// optimization nobody can turn off is one that gets reverted wholesale the
/// first time anything looks wrong.
///
/// Selected once from `OMOYA_TRUEDAMAGE` at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Compute nothing. Byte-for-byte the behaviour that shipped before this
    /// module existed — the client's declaration reaches the renderer intact.
    Off,
    /// Compute, and REPLACE the client's declaration. The fast path.
    On,
    /// ★ Compute, publish the counters, and **throw the answer away** — the
    /// client's declaration still reaches the renderer untouched.
    ///
    /// This is the honest A/B: it pays the comparison so `td_dirty_pct` is a
    /// real measurement of what the refinement *would* have saved, while what
    /// reaches the screen is identical to `Off`. If an artifact persists in
    /// `Verify` it was never this module's, and that is a conclusion no amount
    /// of reading the diff can reach.
    Verify,
}

impl Mode {
    /// Resolve once, from `OMOYA_TRUEDAMAGE`.
    ///
    /// Unknown values fall back to `On` **with a warning** rather than to
    /// `Off`: a typo'd kill-switch that silently disables an optimization is a
    /// performance regression nobody can find, and one that silently enables it
    /// is at least the state the operator was already in.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("OMOYA_TRUEDAMAGE").as_deref() {
            Ok("off" | "0" | "false") => Self::Off,
            Ok("verify") => Self::Verify,
            Ok("on" | "1" | "true") | Err(_) => Self::On,
            Ok(other) => {
                tracing::warn!(
                    value = other,
                    "OMOYA_TRUEDAMAGE is not off|on|verify — defaulting to on"
                );
                Self::On
            }
        }
    }

    /// The wire encoding shared with `OmoyaIntrospect::td_mode`.
    ///
    /// ★ ONE VALUE, NOT TWO. The atomic IS the mode — `Omoya` reads it at each
    /// commit rather than holding its own copy — so "what the seat is doing"
    /// and "what the seat reports" cannot disagree. A cached second copy is
    /// exactly how a live-tunable knob ends up published as flipped while the
    /// hot path still reads the old value.
    #[must_use]
    pub fn to_u64(self) -> u64 {
        match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Verify => 2,
        }
    }

    /// Decode the wire value. Anything unrecognised is `On`, matching
    /// `from_env`'s reasoning: a bad value must not silently disable an
    /// optimization nobody can then find.
    #[must_use]
    pub fn from_u64(v: u64) -> Self {
        match v {
            0 => Self::Off,
            2 => Self::Verify,
            _ => Self::On,
        }
    }

    /// Parse an operator-supplied name, for the live setter.
    ///
    /// # Errors
    /// Returns the accepted set when the name is not one of them — a typed
    /// refusal, so a typo cannot silently select a mode the operator did not
    /// ask for while they watch for an effect that never comes.
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        match s {
            "off" => Ok(Self::Off),
            "on" => Ok(Self::On),
            "verify" => Ok(Self::Verify),
            _ => Err("td_mode must be one of: off, on, verify"),
        }
    }

    /// The operator-facing name, for anything that reports the mode.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Verify => "verify",
        }
    }

    /// Whether the refined damage is allowed to reach the renderer.
    #[must_use]
    pub fn replaces_declaration(self) -> bool {
        matches!(self, Self::On)
    }

    /// Whether to run the comparison at all.
    #[must_use]
    pub fn computes(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Replace a surface's declared damage with what actually changed.
///
/// ★ **MUST RUN BEFORE `on_commit_buffer_handler`.** That is what drains
/// `SurfaceAttributes::damage` into the renderer state
/// (`smithay .../renderer/utils/wayland.rs`, `attrs.damage.drain(..)`), and
/// once it has, the declaration is gone. Running after would edit a field
/// nothing reads and look exactly like working.
///
/// Returns the verdict for the counters, or `None` when there was nothing to
/// decide — no new buffer, not shm, or the client already declared honest
/// damage.
pub fn refine_commit(
    surface: &WlSurface,
    shadows: &mut Shadows,
    mode: Mode,
) -> Option<Verdict> {
    if !mode.computes() {
        return None;
    }
    use smithay::reexports::wayland_server::Resource as _;
    use smithay::utils::{Rectangle, Size};
    use smithay::wayland::compositor::{BufferAssignment, Damage, SurfaceAttributes, with_states};
    use smithay::wayland::shm;

    let key = surface.id().protocol_id();

    with_states(surface, |states| {
        let mut guard = states.cached_state.get::<SurfaceAttributes>();
        let attrs = guard.current();

        // Only a NEW buffer can have new pixels. A commit that reattaches
        // nothing changes state we do not model here.
        let Some(BufferAssignment::NewBuffer(buffer)) = attrs.buffer.as_ref() else {
            return None;
        };
        if attrs.damage.is_empty() {
            return None;
        }

        // `with_buffer_contents` is the only sanctioned way to read an shm
        // pool, and it returns Err for anything that is not one — a dmabuf
        // client simply takes the `None` path.
        let outcome = shm::with_buffer_contents(buffer, |ptr, len, data| {
            #[allow(clippy::cast_sign_loss)]
            let (stride, height, offset) =
                (data.stride as usize, data.height as usize, data.offset as usize);
            #[allow(clippy::cast_sign_loss)]
            let width = data.width as i32;
            let needed = stride.checked_mul(height)?;
            if offset.checked_add(needed)? > len {
                return None;
            }

            // ★ THE GUARD THAT KEEPS THIS FROM TAXING WELL-BEHAVED CLIENTS.
            // The comparison costs a full read of both buffers. A client that
            // already declares honest, small damage would pay that for nothing
            // — so refine only when the declaration is big enough that the
            // read can plausibly pay for itself. wgpu clients declare the
            // whole surface, which is the case this exists for; a client
            // declaring a tenth of its surface is left alone.
            let declared: usize = attrs
                .damage
                .iter()
                .map(|d| match d {
                    Damage::Buffer(r) => (r.size.w.max(0) as usize) * (r.size.h.max(0) as usize),
                    Damage::Surface(r) => (r.size.w.max(0) as usize) * (r.size.h.max(0) as usize),
                })
                .sum();
            #[allow(clippy::cast_sign_loss)]
            let area = (width.max(0) as usize) * height;
            if area == 0 || declared * 2 < area {
                return None;
            }

            // SAFETY: `with_buffer_contents` guarantees `ptr` is valid for
            // `len` bytes for the duration of this closure, and the range is
            // bounds-checked above.
            let bytes = unsafe { std::slice::from_raw_parts(ptr.add(offset), needed) };
            Some(shadows.refine(key, bytes, stride, height, data.format as u32))
        });

        let Ok(Some(verdict)) = outcome else {
            return None;
        };

        // Only ever SHRINK, and only on a verdict that actually compared
        // something. A refusal leaves the client's declaration exactly as it
        // was — see this module's header on why that direction is the safe one.
        if mode.replaces_declaration()
            && let Verdict::Refined { spans, .. } = &verdict
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let w = shm::with_buffer_contents(buffer, |_, _, d| d.width).unwrap_or(0);
            attrs.damage = spans
                .iter()
                .map(|s| {
                    Damage::Buffer(Rectangle::new(
                        (0, s.start as i32).into(),
                        Size::from((w, (s.end - s.start) as i32)),
                    ))
                })
                .collect();
        }
        Some(verdict)
    })
}
