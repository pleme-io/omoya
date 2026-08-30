//! rouka (廊下) — the typed presentation route.
//!
//! The corridor: **by what route do a surface's pixels reach the display, what
//! does that route require, and what does it cost?**
//!
//! Not *what changed* (that is damage), not *may I partial-paint into this
//! target* (`mekuri::kentou`), not *is a frame owed* (`mekuri`). The **route**
//! — the question nothing owned, which is why both of the expensive incidents
//! below were the same defect wearing different clothes.
//!
//! ── ★ WHY THIS EXISTS: TWO MEASUREMENTS ─────────────────────────────────────
//!
//! **2026-08-21.** omoya advertised `zwp_linux_dmabuf_v1`. mado believed the
//! advertisement and switched to GPU buffers. omoya's importer maps the buffer
//! and `to_vec`s it, so for a GPU client that is a CPU readback across PCIe:
//! `gather_us 693 952` — 694 ms per frame, against 3 825 µs of compositing.
//! The global was withdrawn behind an env var and a paragraph.
//!
//! **2026-08-30.** With shm restored, a commit-caused frame costs 4194 µs to a
//! pointer-caused frame's 250 µs — 17×, past the 2.78 ms vblank at 360 Hz. The
//! gap is import: ~7.9 MB copied per keystroke.
//!
//! Both are one bind: **omoya composites on the CPU and its clients render on
//! the GPU**, so every pixel crosses that boundary. shm makes the client pay;
//! dmabuf made the compositor pay. There is no good side of the trade — the
//! exit is a third route (direct scanout) where nobody touches the pixels.
//!
//! ── ★ WHAT IS SEALED HERE, AND WHAT IS NOT ──────────────────────────────────
//!
//! Sealed: a capability cannot be advertised unless the route serving it costs
//! the CPU zero bytes (`Advertisement` has no public constructor), a route
//! cannot name a plane that was not proven scanout-capable (`PlaneLease` has
//! none either), and cost cannot be stated in a unit that hides a copy (`Cost`
//! holds bytes and nothing else).
//!
//! NOT sealed: choosing well. rouka makes a bad route *unrepresentable to
//! promise*; it does not make it unrepresentable to take. Taking a
//! `CpuReadback` route is legal and sometimes correct — what is illegal is
//! telling a client we serve it well.

use core::fmt;

/// Why a route had to fall back to moving pixels through the CPU.
///
/// Closed on purpose: a reason not listed here has no expressible path, so a
/// new fallback cannot be added without naming itself in the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadbackReason {
    /// The compositor has no GPU renderer, so an imported GPU buffer must be
    /// read back to system memory before it can be composited. This is omoya
    /// today (`drm.rs` — "no dmabuf import, no GPU driver dependency").
    CompositorIsCpu,
    /// Client and compositor share no format/modifier that both can address.
    NoSharedFormat,
}

impl fmt::Display for ReadbackReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CompositorIsCpu => "the compositor has no GPU renderer",
            Self::NoSharedFormat => "no format both sides can address",
        })
    }
}

/// A display plane, proven scanout-capable and exclusively held.
///
/// ★ NO PUBLIC CONSTRUCTOR, and deliberately **not** `Copy`. Both facts are
/// load-bearing:
///
/// * no constructor ⇒ the only way to hold one is [`PlaneTable::lease`], which
///   checks the format and modifier — so R2 (a route whose preconditions are
///   unmet) is a construction error rather than a page-flip failure at 3 a.m.
/// * not `Copy` ⇒ two surfaces cannot both hold the same plane (R3), because
///   handing it to one route moves it out of the other.
#[derive(Debug, PartialEq, Eq)]
pub struct PlaneLease {
    id: u32,
}

impl PlaneLease {
    /// The plane's KMS object id.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }
}

/// What a surface needs in order to be routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// FourCC of the client's buffer.
    pub fourcc: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Bytes the surface occupies — the honest denominator for [`Cost`].
    pub bytes: u64,
}

/// The seam. Plane availability is hardware state, so it is injected rather
/// than read, and the whole route decision is provable against a mock.
pub trait PlaneSource {
    /// Lease a plane able to scan out `req`, or `None` if none can.
    fn lease(&mut self, req: &Requirement) -> Option<PlaneLease>;
}

/// A table of real KMS planes.
#[derive(Debug, Default)]
pub struct PlaneTable {
    /// Planes that are free, paired with the (fourcc, modifier) pairs each can
    /// scan out. A plane absent from this list is not offered — "unknown
    /// capability" and "no capability" deliberately render the same, because
    /// promising on an unverified plane is the bug this module exists to stop.
    free: Vec<(u32, Vec<(u32, u64)>)>,
}

impl PlaneTable {
    /// Build from probed hardware: `(plane_id, supported (fourcc, modifier))`.
    #[must_use]
    pub fn new(planes: Vec<(u32, Vec<(u32, u64)>)>) -> Self {
        Self { free: planes }
    }

    /// How many planes remain unleased.
    #[must_use]
    pub fn free_count(&self) -> usize {
        self.free.len()
    }
}

impl PlaneSource for PlaneTable {
    fn lease(&mut self, req: &Requirement) -> Option<PlaneLease> {
        let idx = self
            .free
            .iter()
            .position(|(_, caps)| caps.contains(&(req.fourcc, req.modifier)))?;
        let (id, _) = self.free.remove(idx);
        Some(PlaneLease { id })
    }
}

/// How a surface's pixels reach the display.
///
/// `CpuReadback` is kept **representable on purpose**. It is a real route and
/// sometimes the only one; hiding it is precisely how it got chosen silently in
/// August. What rouka forbids is *promising* it (see [`Advertisement`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// KMS scans the client's buffer out of a plane. Nobody touches the pixels.
    DirectScanout { plane: PlaneLease },
    /// Imported and composited by a GPU renderer. Costs bandwidth, but no CPU
    /// copy — omoya has no such renderer today, so nothing constructs this yet.
    GpuComposited { bytes: u64 },
    /// Mapped into system memory and copied. The 694 ms trap.
    CpuReadback { reason: ReadbackReason, bytes: u64 },
}

/// What a route costs, in **bytes the CPU must move per frame**.
///
/// ★ THE UNIT IS THE POINT. This was measured and reported for months as
/// "frames per second" and "percent of a core", and in those units 7.9 MB per
/// keystroke is invisible — it reads as a slow frame, not as eight megabytes.
/// A type that cannot express a percentage cannot hide a copy behind one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    /// Bytes the CPU moves per frame on this route.
    pub cpu_bytes_per_frame: u64,
}

impl Cost {
    /// True when the CPU moves no pixels at all.
    #[must_use]
    pub const fn is_zero_copy(&self) -> bool {
        self.cpu_bytes_per_frame == 0
    }
}

impl Route {
    /// This route's cost. Total for `CpuReadback`, zero for the other two —
    /// `GpuComposited` moves bytes, but not across the CPU, and the CPU is the
    /// axis every incident here was on.
    #[must_use]
    pub const fn cost(&self) -> Cost {
        Cost {
            cpu_bytes_per_frame: match self {
                Self::DirectScanout { .. } | Self::GpuComposited { .. } => 0,
                Self::CpuReadback { bytes, .. } => *bytes,
            },
        }
    }

    /// A short stable label for recording which route a frame took (R7).
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::DirectScanout { .. } => "direct-scanout",
            Self::GpuComposited { .. } => "gpu-composited",
            Self::CpuReadback { .. } => "cpu-readback",
        }
    }
}

/// Refusals. Every variant names what was attempted, so an error is a
/// diagnosis rather than a notification.
#[derive(Debug, PartialEq, Eq)]
pub enum RoukaError {
    /// R1: a promise we cannot keep.
    PromisedWhatWeServeBadly {
        /// The label of the offending route.
        route: &'static str,
        /// What the CPU would move per frame if we kept the promise.
        cpu_bytes_per_frame: u64,
    },
}

impl fmt::Display for RoukaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PromisedWhatWeServeBadly {
                route,
                cpu_bytes_per_frame,
            } => write!(
                f,
                "refusing to advertise a capability served by `{route}`: it moves \
                 {cpu_bytes_per_frame} CPU bytes per frame. A protocol global is a \
                 PROMISE about what we do well, and a client cannot discover that \
                 our implementation is a readback — it optimises for the promise."
            ),
        }
    }
}

impl core::error::Error for RoukaError {}

/// A protocol capability we are willing to promise to clients.
///
/// ★ THE LOAD-BEARING INVARIANT OF THIS MODULE. There is no public
/// constructor and the field is private, so the ONLY way to obtain an
/// `Advertisement` is [`Advertisement::try_promise`], which refuses any route
/// that is not zero-copy.
///
/// That turns the August incident from a thing a reader must remember into a
/// thing the compiler will not let them express. Before: a global guarded by
/// `OMOYA_ADVERTISE_DMABUF` and a 30-line comment asking the next person to be
/// careful. After: `try_promise(CpuReadback { .. })` returns `Err` and there is
/// no other door.
#[derive(Debug, PartialEq, Eq)]
pub struct Advertisement(Route);

impl Advertisement {
    /// Promise a capability, if and only if the route serving it is zero-copy.
    ///
    /// # Errors
    /// [`RoukaError::PromisedWhatWeServeBadly`] when the route moves CPU bytes.
    pub fn try_promise(route: Route) -> Result<Self, RoukaError> {
        let cost = route.cost();
        if cost.is_zero_copy() {
            Ok(Self(route))
        } else {
            Err(RoukaError::PromisedWhatWeServeBadly {
                route: route.label(),
                cpu_bytes_per_frame: cost.cpu_bytes_per_frame,
            })
        }
    }

    /// The route backing this promise.
    #[must_use]
    pub const fn route(&self) -> &Route {
        &self.0
    }
}

/// Choose the best available route for a surface.
///
/// Prefers direct scanout; falls back to a readback that NAMES its reason. The
/// fallback is deliberately not silent at the type level — the caller receives
/// a `Route` it can record (R7) and, once M2 lands, expose.
pub fn choose(source: &mut impl PlaneSource, req: &Requirement, cpu_composited: bool) -> Route {
    if let Some(plane) = source.lease(req) {
        return Route::DirectScanout { plane };
    }
    Route::CpuReadback {
        reason: if cpu_composited {
            ReadbackReason::CompositorIsCpu
        } else {
            ReadbackReason::NoSharedFormat
        },
        bytes: req.bytes,
    }
}

/// A candidate surface for direct scanout, in output-physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// Where the surface lands on the output.
    pub x: i32,
    /// Y position on the output.
    pub y: i32,
    /// Width in physical pixels.
    pub w: i32,
    /// Height in physical pixels.
    pub h: i32,
    /// Buffer dimensions. Differing from `w`/`h` means the compositor is
    /// SCALING, which a plane without a scaler cannot reproduce.
    pub buffer_w: i32,
    /// Buffer height.
    pub buffer_h: i32,
    /// Whether every pixel is opaque. A plane composites below everything
    /// drawn by the CPU path, so a translucent surface would show the wrong
    /// thing behind it.
    pub opaque: bool,
}

/// Why a candidate cannot be scanned out.
///
/// ★ CLOSED, AND EVERY ARM IS A REAL KMS CONSTRAINT. A `bool` here would be
/// the R5 mistake one level up: "not eligible" is unactionable, while
/// "`Scaled`" tells the caller to stop scaling and "`Occluded`" tells it to
/// try again when the overlap clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
    /// No plane advertises this (fourcc, modifier).
    UnsupportedFormat,
    /// Destination differs from buffer size and no scaler was proven.
    Scaled,
    /// Something the compositor draws lands on top of this rectangle.
    Occluded,
    /// Translucent, so what shows through the plane would be wrong.
    Translucent,
    /// Degenerate geometry.
    Empty,
}

/// Decide whether `cand` may be handed to a plane, given what the compositor
/// itself draws on top (`occluders`).
///
/// ── ★ OCCLUSION IS THE ONE THAT BITES ───────────────────────────────────────
///
/// The format and scaling checks fail loudly at commit time if you get them
/// wrong. Occlusion does not: a plane composites UNDER the primary, so an
/// overlapping element still draws correctly — until the frame where the
/// compositor skips repainting that region because nothing damaged it, and the
/// plane's content shows through a hole that should have been covered. That is
/// the stale-pixel class again, arriving from a new direction, which is why it
/// is checked here rather than trusted to the caller.
///
/// Measured on plo 2026-08-30, and it is why this is worth having: the bar sits
/// at `0,0 1920x28`, mado's window at `518,280 883x547`, and the four focus-ring
/// edges surround rather than cover it. Nothing overlaps, so mado is eligible —
/// a fact no amount of reading the compositor would have told you.
///
/// # Errors
/// [`Ineligible`] naming which constraint failed.
pub fn eligible_for_plane(
    cand: &Candidate,
    req: &Requirement,
    occluders: &[(i32, i32, i32, i32)],
    plane_formats: &[(u32, u64)],
) -> Result<(), Ineligible> {
    if cand.w <= 0 || cand.h <= 0 {
        return Err(Ineligible::Empty);
    }
    if !cand.opaque {
        return Err(Ineligible::Translucent);
    }
    if cand.w != cand.buffer_w || cand.h != cand.buffer_h {
        return Err(Ineligible::Scaled);
    }
    if !plane_formats.contains(&(req.fourcc, req.modifier)) {
        return Err(Ineligible::UnsupportedFormat);
    }
    let (l, t, r, b) = (cand.x, cand.y, cand.x + cand.w, cand.y + cand.h);
    for &(ox, oy, ow, oh) in occluders {
        if ow <= 0 || oh <= 0 {
            continue;
        }
        // Half-open intervals: an occluder whose edge merely TOUCHES the
        // candidate shares no pixel. Using closed intervals here would reject
        // plo's focus ring, which abuts the window exactly.
        if ox < r && ox + ow > l && oy < b && oy + oh > t {
            return Err(Ineligible::Occluded);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARGB: u32 = 0x3443_2241;
    const LINEAR: u64 = 0;
    /// 1920x1080x4 — the real per-keystroke figure from plo.
    const FULLSCREEN_BYTES: u64 = 1920 * 1080 * 4;

    fn req() -> Requirement {
        Requirement {
            fourcc: ARGB,
            modifier: LINEAR,
            bytes: FULLSCREEN_BYTES,
        }
    }

    /// ★ THE HISTORICAL BUG, AS A TEST. This is 2026-08-21 exactly: a dmabuf
    /// global advertised while the serving route was a CPU readback.
    #[test]
    fn advertising_a_readback_route_is_refused() {
        let err = Advertisement::try_promise(Route::CpuReadback {
            reason: ReadbackReason::CompositorIsCpu,
            bytes: FULLSCREEN_BYTES,
        })
        .expect_err("a readback route must never become a promise");
        assert_eq!(
            err,
            RoukaError::PromisedWhatWeServeBadly {
                route: "cpu-readback",
                cpu_bytes_per_frame: FULLSCREEN_BYTES,
            }
        );
        // The refusal names the number, so nobody has to go measure it again.
        assert!(err.to_string().contains("8294400"));
    }

    #[test]
    fn a_scanout_route_may_be_promised() {
        let mut table = PlaneTable::new(vec![(31, vec![(ARGB, LINEAR)])]);
        let route = choose(&mut table, &req(), true);
        assert_eq!(route.label(), "direct-scanout");
        assert!(route.cost().is_zero_copy());
        assert!(Advertisement::try_promise(route).is_ok());
    }

    /// R2: a plane that cannot scan out this format is never leased, so the
    /// route degrades to a readback instead of failing at page-flip time.
    #[test]
    fn a_plane_that_cannot_scan_out_the_format_is_not_leased() {
        let mut table = PlaneTable::new(vec![(31, vec![(ARGB, 0x0100_0000)])]);
        let route = choose(&mut table, &req(), true);
        assert_eq!(route.label(), "cpu-readback");
        assert_eq!(table.free_count(), 1, "the plane must stay available");
    }

    /// R3: one plane cannot back two surfaces. `PlaneLease` is not `Copy`, so
    /// the first route moves it out of the table and the second cannot have it.
    #[test]
    fn one_plane_cannot_back_two_surfaces() {
        let mut table = PlaneTable::new(vec![(31, vec![(ARGB, LINEAR)])]);
        let first = choose(&mut table, &req(), true);
        let second = choose(&mut table, &req(), true);
        assert_eq!(first.label(), "direct-scanout");
        assert_eq!(second.label(), "cpu-readback");
        assert_eq!(table.free_count(), 0);
    }

    /// R5: cost is bytes. There is no constructor taking a percentage or a
    /// frame count, which is what let 7.9 MB read as "a slow frame" for months.
    #[test]
    fn cost_is_expressed_in_bytes_and_a_readback_is_never_zero() {
        let readback = Route::CpuReadback {
            reason: ReadbackReason::CompositorIsCpu,
            bytes: FULLSCREEN_BYTES,
        };
        assert_eq!(readback.cost().cpu_bytes_per_frame, 8_294_400);
        assert!(!readback.cost().is_zero_copy());
    }

    /// The fallback names WHY, so a route recorded in a log is a diagnosis.
    #[test]
    fn the_fallback_names_its_reason() {
        let mut empty = PlaneTable::default();
        match choose(&mut empty, &req(), true) {
            Route::CpuReadback { reason, .. } => {
                assert_eq!(reason, ReadbackReason::CompositorIsCpu);
                assert_eq!(reason.to_string(), "the compositor has no GPU renderer");
            }
            other => panic!("expected a readback, got {}", other.label()),
        }
    }

    fn cand(x: i32, y: i32, w: i32, h: i32) -> Candidate {
        Candidate {
            x,
            y,
            w,
            h,
            buffer_w: w,
            buffer_h: h,
            opaque: true,
        }
    }

    /// ★ THE REAL plo LAYOUT, from `geometry` on the live seat 2026-08-30.
    /// The bar does not reach the window and the focus ring only abuts it, so
    /// mado IS eligible — the measurement that makes M3b worth building.
    #[test]
    fn the_measured_plo_layout_is_eligible_for_a_plane() {
        let occluders = [
            (0, 0, 1920, 28),    // the bar
            (516, 278, 887, 2),  // focus ring: top
            (516, 827, 887, 2),  // bottom
            (516, 280, 2, 547),  // left
            (1401, 280, 2, 547), // right
        ];
        assert_eq!(
            eligible_for_plane(
                &cand(518, 280, 883, 547),
                &req(),
                &occluders,
                &[(ARGB, LINEAR)]
            ),
            Ok(())
        );
    }

    /// An occluder that merely ABUTS shares no pixel. Closed intervals here
    /// would reject plo's real layout, so this pins the half-open rule.
    #[test]
    fn an_abutting_edge_does_not_occlude() {
        assert_eq!(
            eligible_for_plane(
                &cand(100, 100, 50, 50),
                &req(),
                &[(50, 100, 50, 50)], // ends exactly at x=100
                &[(ARGB, LINEAR)]
            ),
            Ok(())
        );
    }

    /// One pixel of real overlap is a refusal.
    #[test]
    fn a_single_overlapping_pixel_refuses() {
        assert_eq!(
            eligible_for_plane(
                &cand(100, 100, 50, 50),
                &req(),
                &[(51, 100, 50, 50)], // ends at x=101, overlaps by 1
                &[(ARGB, LINEAR)]
            ),
            Err(Ineligible::Occluded)
        );
    }

    /// Each constraint is reported distinctly — the reason is the payload.
    #[test]
    fn each_refusal_names_its_own_constraint() {
        let ok_fmt = [(ARGB, LINEAR)];
        let mut scaled = cand(0, 0, 100, 100);
        scaled.buffer_w = 50;
        let mut clear = cand(0, 0, 100, 100);
        clear.opaque = false;
        assert_eq!(
            eligible_for_plane(&scaled, &req(), &[], &ok_fmt),
            Err(Ineligible::Scaled)
        );
        assert_eq!(
            eligible_for_plane(&clear, &req(), &[], &ok_fmt),
            Err(Ineligible::Translucent)
        );
        assert_eq!(
            eligible_for_plane(&cand(0, 0, 100, 100), &req(), &[], &[]),
            Err(Ineligible::UnsupportedFormat)
        );
        assert_eq!(
            eligible_for_plane(&cand(0, 0, 0, 10), &req(), &[], &ok_fmt),
            Err(Ineligible::Empty)
        );
    }

    /// ANTI-VACUITY. If `try_promise` accepted everything these tests would
    /// still pass on the happy path, so assert the guard actually discriminates
    /// — one route in, one route out, from the same call.
    #[test]
    fn the_guard_discriminates_rather_than_accepting_everything() {
        let mut table = PlaneTable::new(vec![(31, vec![(ARGB, LINEAR)])]);
        let good = choose(&mut table, &req(), true);
        let bad = choose(&mut table, &req(), true);
        assert!(Advertisement::try_promise(good).is_ok());
        assert!(Advertisement::try_promise(bad).is_err());
    }
}
