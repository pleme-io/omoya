//! omoya's introspection plane — what an agent can ask a RUNNING compositor.
//!
//! ── WHY kanshou AND NOT A BESPOKE SOCKET ──────────────────────────────────
//! The fleet already solved "a long-running process an operator cannot see
//! into". `kanshou` (観照) is a shipped library — published 0.1.6, ~1500 lines,
//! 27 tests, and consumed BY VERSION by mado, tear and frost. It gives a
//! process a Unix-socket sidecar keyed on (app, pid), an `Introspect` trait, a
//! discovery function, an operator CLI (`gen kanshou list|query|schema`), and
//! an MCP bridge that tags every answer with whether it came from the LIVE
//! process or a fallback.
//!
//! Writing a fresh socket here would have been the 22nd bespoke surface, and it
//! would have started by reinventing the thing kanshou exists to prevent: mado
//! once reported `frame_perf 0` over MCP while its GUI rendered at 120fps,
//! because the MCP server had no wire into the live process. omoya would have
//! hit exactly that.
//!
//! ── ★ OBSERVE ONLY, AND THAT IS A DECISION NOT A STAGE ────────────────────
//! Every leaf here is a READ. Nothing in this file can change the seat.
//!
//! That is deliberate and it is not merely caution about an unfinished feature.
//! An MCP surface on a compositor can black the screen of a machine someone is
//! sitting at — which is not hypothetical: it happened on plo during this
//! session's development, and the operator watched it. mado ships 63 MCP tools
//! with no legality gate at all, and the fleet's typed gate for this
//! (`postigo`'s `ActionLegality`) is real but lives INSIDE banken and is not a
//! library anything else can consume — measured, not assumed.
//!
//! So mutation waits for a gate rather than arriving before one. `banken`'s own
//! rule is the precedent: it ships a read-only MCP server with a CI guard
//! asserting no mutating tool exists, and carries `pending-banken: mcp-declare`
//! rather than shipping the verb early.
//!
//! `pending-omoya-mutate:` mutating leaves need (a) a typed legality gate and
//! (b) a calloop WAKEUP source. Note (b) specifically: `Introspect::query`
//! takes `&self` and runs on the sidecar's thread, while omoya's state lives in
//! a single-threaded calloop loop. mado's `InjectedActions` queue is the right
//! shape but not copyable verbatim — mado drains per GUI frame, whereas a
//! compositor that is idle is not rendering, so a queued action would sit
//! unnoticed until something else woke the loop. omoya needs
//! `calloop::ping::make_ping` so the enqueue itself wakes the loop.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use kanshou::{Introspect, Query, QueryError, QueryResult};

/// The snapshot omoya publishes.
///
/// Atomics rather than a lock around the compositor state, for a reason that is
/// structural rather than stylistic: the sidecar answers on ITS OWN thread while
/// the compositor runs a single-threaded event loop. Sharing the real `Omoya`
/// behind a mutex would let an agent's question block a frame — an introspection
/// surface that can stutter the thing it observes is worse than none.
///
/// So the loop PUBLISHES cheap facts as it goes, and the sidecar reads those.
/// The cost is that answers are as fresh as the last tick rather than exact at
/// the instant asked; for "is it rendering, what is it rendering on, how many
/// clients" that is the right trade, and it is stated so nobody reads these as
/// transactional.
/// A pending capture: which id asked, where to write, and what region.
///
/// ★ Carries the id so the RESULT can carry it too. Without that a result is
/// anonymous and a client cannot tell its own answer from a predecessor's.
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub id: u64,
    pub path: String,
    /// `None` = the whole output. `Some((x, y, w, h))` clips.
    ///
    /// ★ The renderer's `copy_region` already takes and clips a rectangle
    /// (`nuri_renderer.rs`); only `drm.rs::capture` hardcoded full-size. The
    /// region path is wiring, not new readback machinery.
    pub region: Option<(i32, i32, i32, i32)>,
    /// Hash the pixels instead of writing them.
    ///
    /// ★ Measured on plo: three captures 0.5s apart are BIT-IDENTICAL, and
    /// across 70s only the top 28 rows differ -- the 1 Hz clock in the bar.
    /// So a hash is a viable change-oracle, and masking the bar is mandatory
    /// or every comparison differs for a reason nobody cares about.
    pub hash_only: bool,
}

/// One toplevel, with the protocol side and the pixel side in the same row.
///
/// ★ `Option` is load-bearing on the P-side field: "the client was told
/// ServerSide" and "the client has not been told anything yet" are different
/// answers, and a default would collapse them into the first -- the round-up
/// that makes a race look like a policy.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToplevelRow {
    /// Stable within a run. Not a surface pointer: those get reused.
    pub id: u64,
    pub app_id: Option<String>,
    /// `"ServerSide"` / `"ClientSide"`, or `None` if no decoration object exists.
    pub decoration_mode_sent: Option<String>,
    /// The window's rect in the Space, `(x, y, w, h)`.
    pub rect: Option<(i32, i32, i32, i32)>,
    /// Chrome elements the compositor drew FOR THIS WINDOW.
    ///
    /// ★ Zero is the whole finding when `decoration_mode_sent` is
    /// `ServerSide`: the client drew nothing because it was told not to, and
    /// the compositor drew nothing because only the focused window gets a ring.
    pub decoration_elements_drawn: u32,
    pub focused: bool,
    /// Whether the layout tree holds this window. `false` in floating mode for
    /// EVERY window (`layout.rs` unmaps them all), which is why a resize deed
    /// is a silent no-op there independently of the `move_request` stubs.
    pub tiled: bool,
}

impl ToplevelRow {
    /// The one-line verdict a reader actually wants.
    ///
    /// ★ Names the CONJUNCTION. Each input is individually legal; the
    /// combination is the defect, and no per-field view can show it.
    #[must_use]
    pub fn chrome_verdict(&self, floating: bool) -> &'static str {
        let told_server = self.decoration_mode_sent.as_deref() == Some("ServerSide");
        match (told_server, floating, self.decoration_elements_drawn) {
            (true, true, 0) => "no-grabbable-chrome: told ServerSide, floating, none drawn",
            (true, _, 0) => "server-side promised, none drawn",
            (true, _, _) => "server-side drawn",
            (false, _, _) => "client-side (client draws its own)",
        }
    }
}

#[derive(Debug, Default)]
pub struct OmoyaIntrospect {
    /// Which backend is driving pixels: 0 = nested (winit), 1 = drm.
    pub backend: AtomicU64,
    /// Whether the DRM surface drives ATOMIC or LEGACY modesetting.
    /// 0 = not yet known, 1 = atomic, 2 = legacy.
    ///
    /// ── ★ WHY THIS LEAF EXISTS ──────────────────────────────────────────
    /// It was the sharpest UNMEASURED fact about this compositor: 44 read
    /// leaves and not one of them recorded which commit path the seat is on,
    /// while `DrmSurface::is_atomic()` sat there uncalled. That matters most
    /// on the exact hardware this seat runs — the proprietary nvidia driver
    /// with `nvidia-drm.modeset=1` and DUMB buffers — where the legacy and
    /// atomic flip paths are not the same code in the kernel and do not fail
    /// the same way.
    ///
    /// Diagnosing a "tearing" report without it means guessing at which of
    /// two kernel paths is running. A leaf is cheaper than the guess.
    pub atomic: AtomicU64,
    /// Frames the render loop has queued since start.
    pub frames: AtomicU64,
    /// Frames actually PRESENTED — i.e. page-flipped to the display.
    ///
    /// ★ THE DENOMINATOR FOR THE PARTIAL-REPAINT CLAIM, AND THE REASON IT IS
    /// A CLAIM AND NOT AN ASSERTION. `frames` counts render-loop ticks and
    /// goes up whether or not anything changed, so it cannot distinguish "we
    /// now skip idle frames" from "we still composite the whole screen sixty
    /// times a second". Damage tracking is exactly the kind of optimisation
    /// that can be wired up, look correct in a screenshot, and quietly do
    /// nothing — a stale element `Id` or a mis-derived buffer age turns every
    /// frame into a full repaint while every pixel stays right.
    ///
    /// `presented < frames` on an idle seat is the measurement that says it is
    /// working. Equal counts say it is not.
    pub presented: AtomicU64,

    // ── ★ WHAT THE CLIENT ACTUALLY DECLARED ──────────────────────────────
    //
    // The stale-pixel hunt of 2026-08-30 stalled on a question no counter
    // could answer: mado's own `grid_damage.rs` asserts it never calls
    // `wl_surface.damage_buffer`, so "the whole surface is reported dirty on
    // every present". If that were true the negative-control probe would have
    // measured ZERO stale pixels. It measured 34.
    //
    // One of the two is wrong and NO amount of reading settles it, because the
    // claim is about what a foreign library (wgpu/Mesa WSI) emits at run time,
    // not about our source. So: count it at the seam where it arrives.
    //
    // `empty` is the interesting one. An import with empty damage takes the
    // full-copy path (`nuri_renderer` :966), so an all-empty distribution
    // means the texture is always complete and the under-report is downstream
    // in ELEMENT damage; a mixed distribution means the client is declaring
    // fine-grained damage and mado's comment is stale.
    /// shm imports serviced, total.
    pub shm_imports: AtomicU64,
    /// …of which arrived with EMPTY client damage (⇒ full copy).
    pub shm_imports_empty_damage: AtomicU64,
    /// Damage rectangles in the most recent import.
    pub shm_damage_rects: AtomicU64,
    /// Total damaged area, in pixels, in the most recent import.
    pub shm_damage_area: AtomicU64,

    // ── ★ THE COST, IN THE ONE UNIT THAT MAKES IT ALARMING ───────────────
    //
    // This number existed all along as "frames per second" and "percent of a
    // core", and in those units 7.9 MB per keystroke is INVISIBLE — it reads
    // as a slow frame. `rouka::Cost` holds bytes and nothing else for exactly
    // this reason; these leaves are that type made observable at runtime.
    /// Bytes the CPU moved for the most recent import.
    pub route_cpu_bytes: AtomicU64,
    /// Bytes the CPU has moved across every import since start.
    pub route_cpu_bytes_total: AtomicU64,
    /// Which `rouka::Route` the last import took, as its stable label (R7 —
    /// a route nobody records is a regression nobody can see).
    pub route_label: std::sync::Mutex<Option<&'static str>>,

    /// The DRM planes reachable from this CRTC, as JSON.
    ///
    /// ★ M3a — this MEASURES the direct-scanout premise instead of assuming
    /// it. `docs/SMOOTHNESS.md` says plo exposes 12 planes, but 12 planes on
    /// the DEVICE is a different claim from *this CRTC* having a usable
    /// overlay: a plane is reachable only if the CRTC is in its
    /// `possible_crtcs` mask. An empty overlay list here means direct scanout
    /// is dead as designed and the load falls to M5 + M6.
    pub planes: std::sync::Mutex<Option<String>>,
    /// The rectangles the layout last assigned, as `"x,y,wxh"` per window.
    ///
    /// ★ PUBLISHED BECAUSE GUESSING AT A LAYOUT FROM PIXELS IS BACKWARDS.
    /// A screenshot says where windows ENDED UP; it cannot say what the tree
    /// asked for. When those disagree — the tree splits correctly and the
    /// windows still stack — a pixel probe reports "stacked" and gives no way
    /// to tell a broken split from a broken placement from an early return in
    /// `apply_layout`. This is the tree's own answer, read from the same
    /// socket, so the two can be compared instead of one being inferred.
    pub layout: Mutex<Vec<String>>,
    /// The focused window's rectangle, or empty when nothing is focused.
    /// Read by the render loop to draw the focus border, and queryable so an
    /// agent can ask "where is focus" without inferring it from pixels.
    pub focus_rect: Mutex<Option<(i32, i32, i32, i32)>>,
    /// How many RENDER ELEMENTS the last frame actually had.
    ///
    /// ★ NOT THE SAME AS `windows`, AND THE GAP IS THE DIAGNOSIS. `windows`
    /// counts what `Space` holds — a toplevel is in there from the moment it
    /// is created, before the client has attached a single buffer. An element
    /// only exists once there is something to draw. So `windows: 2,
    /// elements: 1` says precisely "a client mapped and never drew", which is
    /// a client-side or protocol problem, while `windows: 2, elements: 2` with
    /// a missing window says the compositor dropped it — a renderer or damage
    /// problem. Those live in different files and a screenshot cannot tell
    /// them apart.
    pub elements: AtomicU64,
    /// Microseconds the last frame spent inside `render_output`.
    ///
    /// ★ ADDED AFTER GUESSING TWICE. The seat sat at 99% CPU with every gdb
    /// sample inside `memmove`, and two separate optimisations — a derived
    /// refresh rate, then a row-wise blit — each looked like the answer and
    /// each moved the number barely. Guessing at a hot spot from the outside
    /// is how that happens; this is the inside.
    pub frame_us: AtomicU64,
    /// Rows the blit took the 1:1 fast path for, and rows it did not.
    /// If `blit_slow` dominates, the fast path's precondition is wrong —
    /// which is invisible from a profile that only says "memmove".
    /// Rows nuri took wholesale via `copy_from_slice`.
    pub blit_fast: AtomicU64,
    /// Rows that fell to per-pixel alpha blending.
    ///
    /// ★ THIS NOW MEANS WHAT IT SAYS. It used to be derived from the OUTER
    /// precondition, which cannot see nuri's per-row opacity test — so a
    /// client whose every row contained one translucent pixel reported
    /// `blit_slow: 0` while blending every row.
    pub blit_slow: AtomicU64,
    /// Pixels through the general transform/scale path, entered when the
    /// outer precondition fails entirely. Counted in PIXELS, not rows,
    /// because that arm has no row structure — a row count there would be a
    /// different unit wearing the same name.
    pub blit_general: AtomicU64,
    /// Microseconds the last frame spent GATHERING elements — which is where
    /// texture import happens, and therefore where a client's buffer is
    /// copied.
    ///
    /// ★ SPLIT FROM `frame_us` BECAUSE `frame_us` EXONERATED THE WRONG HALF.
    /// It measured `render_output` only and read 4 ms while the seat sat at
    /// 99% CPU and 1.4 fps — conclusive proof that compositing was innocent
    /// and equally conclusive that the instrumentation was pointed at the
    /// wrong place.
    pub gather_us: AtomicU64,
    /// Microseconds the last presented frame spent FLUSHING the composed
    /// shadow into the scanout mapping.
    ///
    /// ★ THE THIRD TERM, AND THE ONE THAT WAS INSIDE A TOTAL. `frame_us`
    /// brackets the flush and `gather_us` does not overlap it, so between them
    /// the largest single write in the frame had no name. `flush_damage`
    /// copies stride x height into write-combining memory unconditionally —
    /// 8 294 400 bytes at 1920x1080 — because the partial path is gated behind
    /// a `Target<Known>` nobody constructs yet.
    ///
    /// Read as a fraction of `frame_us`: a flush that dominates says the seat
    /// is bound by one memcpy into uncached memory, which no change detector
    /// upstream can improve.
    pub flush_us: AtomicU64,
    /// The WORST flush seen since start, and the running total.
    ///
    /// ★ BECAUSE `flush_us` ALONE IS A POINT SAMPLE, AND I MIS-READ IT.
    /// Three reads of `flush_us` on plo 2026-09-02 gave 3 729, 7 722 and
    /// 10 873 µs — a 3x spread for what should be a fixed 8 294 400-byte copy.
    /// Quoting any one of them as "the" flush cost is the dated-claim error in
    /// miniature: each reading is honest and none is representative. `max`
    /// bounds the tail and `total / presented` gives the mean, which together
    /// say far more than a fresh sample ever can.
    pub flush_us_max: AtomicU64,
    pub flush_us_total: AtomicU64,
    /// Bytes the last flush actually wrote, and the running total.
    ///
    /// ★ THE DENOMINATOR THAT MAKES `flush_us` INTERPRETABLE, and the reason
    /// this is bytes rather than thread-CPU-time. `flush_us` is WALL CLOCK, so
    /// it cannot by itself separate "the copy was slow" from "the thread was
    /// descheduled mid-copy" — and plo demonstrably runs other heavy work (a
    /// Kolla OpenStack was mid-bring-up during one of the readings above).
    /// `flush_bytes / flush_us` is a RATE, and a rate that collapses while the
    /// byte count holds constant is contention, not work. That distinction is
    /// unavailable from a timer alone.
    ///
    /// It is also the direct measure of what a damage-clipped flush buys:
    /// `Full` writes stride x height every presented frame, `Baselined` writes
    /// only the damage, so the ratio of these two counters IS the saving —
    /// observed rather than predicted.
    pub flush_bytes: AtomicU64,
    pub flush_bytes_total: AtomicU64,
    /// shm imports that copied the WHOLE buffer, versus only their damage.
    /// `import_full` staying high means the incremental path's precondition
    /// never holds — most likely `Arc::get_mut` failing because smithay is
    /// holding the texture too.
    /// ── ★ TRUE-DAMAGE COUNTERS, AND EVERY ONE CARRIES ITS DENOMINATOR ──
    /// `td_rows_dirty` alone cannot distinguish "nothing is changing" from
    /// "nothing is being compared", and those are opposite facts about whether
    /// this optimization is working at all. `td_rows_examined` is the
    /// denominator; `td_refused` is how often the comparison declined and the
    /// frame paid full price.
    /// 0 = off, 1 = on, 2 = verify. Stored rather than re-read from the env
    /// so the published value is what the RUNNING seat resolved, not what an
    /// environment happens to say now.
    /// Every mapped window's `app_id`, in the order the space holds them.
    ///
    /// Published from `apply_layout`, which is the one place that already
    /// walks every window and asks this exact question for the placement
    /// rule — so the published value is BY CONSTRUCTION the value the rule
    /// saw, not a second lookup that could disagree with it.
    pub window_app_ids: std::sync::Mutex<Vec<Option<String>>>,
    pub td_mode: AtomicU64,
    pub td_refined: AtomicU64,
    pub td_refused: AtomicU64,
    pub td_rows_dirty: AtomicU64,
    pub td_rows_examined: AtomicU64,
    /// Shadows held. One per live surface; a number that only grows is a leak
    /// of ~8 MB per window, which otherwise shows up only as RSS.
    pub td_shadows: AtomicU64,
    /// Presents that CLEARED the unpresented ledger — the denominator of the
    /// per-present damage invariant.
    ///
    /// ★ IT EXISTED, IT WAS COUNTED, IT WAS TESTED, AND NOTHING COULD READ IT.
    /// `Shadows::presented_marks` has been declared, incremented on every
    /// `mark_presented()` and unit-tested since the per-present fix landed, and
    /// `publish_truedamage` stored five fields beside it and not this one. So
    /// the invariant "damage from a commit that was never presented is carried
    /// forward, and released only by a real flip" could be argued from source
    /// and never *observed* on a running seat.
    ///
    /// Read it against `td_refined`: refinements that never reached the glass
    /// are `td_refined - td_presented_marks`, which is exactly the backlog the
    /// ledger exists to hold. A gap that grows without bound is a ledger that
    /// is accumulating and never being released; a gap pinned at zero on a
    /// seat that is definitely painting means `mark_presented` is being reached
    /// on a path that never flipped.
    pub td_presented_marks: AtomicU64,
    pub import_full: AtomicU64,
    pub import_partial: AtomicU64,
    /// Each render element's geometry, as the RENDERER sees it.
    ///
    /// ★ THE THIRD INDEPENDENT VIEW OF THE SAME QUESTION. `layout` is what
    /// `Space` holds; this is what `space_render_elements` produced from it.
    /// They are computed by different code from different inputs, so they can
    /// disagree — and if they do, the gap is precisely between "the
    /// compositor's model" and "what the frame was told to draw", which is a
    /// seam no screenshot exposes.
    pub geometry: Mutex<Vec<String>>,
    /// Deeds requested over the socket, waiting for the compositor thread.
    ///
    /// ★ A QUEUE AND NOT A CALL, BECAUSE THE THREADS ARE DIFFERENT.
    /// `Introspect::query` takes `&self` and runs on the sidecar's thread,
    /// while omoya's state lives in a single-threaded calloop loop. Nothing
    /// here may touch `Omoya`; the socket thread enqueues, the compositor
    /// thread drains.
    pub pending_deeds: Mutex<Vec<crate::deed::Deed>>,
    /// Synthetic input awaiting the render thread. Same shape and same reason
    /// as `pending_deeds`: the socket thread may not touch `Omoya`.
    pub pending_input: Mutex<Vec<crate::synth::Synth>>,
    /// How many synthetic steps the COMPOSITOR thread actually performed.
    ///
    /// ★ Written on the far side of the seam, like `deeds_performed`. A
    /// caller that only saw "queued" could not tell an applied keystroke from
    /// one nothing ever drained — which is the exact failure this whole
    /// surface exists to diagnose.
    pub synth_performed: AtomicU64,
    /// Deeds the COMPOSITOR THREAD has actually performed.
    ///
    /// ★ THE READ-BACK FOR THE WRITE SURFACE, AND IT WAS MISSING ONCE.
    /// The `do` leaf answers `queued: <verb>` the instant it pushes, which is
    /// true and says nothing about execution — a deed that nothing drains
    /// answers exactly the same as one that ran. The first version of the
    /// gate asserted on that string and therefore proved only that the socket
    /// was reachable.
    ///
    /// This counter is written on the other side of the seam, by the thread
    /// that does the work, so `queued` and `performed` are two independently
    /// sourced numbers that can disagree. A caller polls it to learn the deed
    /// LANDED rather than that it was accepted.
    pub deeds_performed: AtomicU64,
    /// Deeds performed from a CHORD — the keyboard path.
    ///
    /// ★ Separate from `deeds_performed` on purpose, and the separation is the
    /// point. That counter is incremented only where kanshou-requested deeds
    /// are drained, so it can sit at zero forever while the operator types.
    /// Two paths reaching one action need two counters, or the quiet one is
    /// invisible.
    pub chord_deeds: AtomicU64,
    /// Wakes the compositor's event loop after an enqueue.
    ///
    /// ★ THE PING IS LOAD-BEARING, AND MORE SO SINCE DAMAGE TRACKING LANDED.
    /// mado's `InjectedActions` queue is the right shape but not copyable
    /// verbatim: mado drains once per GUI frame, whereas an idle compositor
    /// is not rendering at all. Measured on vkms — an idle seat now produces
    /// **0 presentations across 183 render ticks** — so a queued deed would
    /// sit unnoticed until something else happened to wake the loop. Which is
    /// to say: on a quiet desktop, exactly when a remote caller most needs it
    /// to work, it would appear to do nothing.
    ///
    /// `OnceLock` because the ping is created with the event loop, after this
    /// struct. A deed enqueued before then is still queued, just not woken —
    /// it lands on the first frame either way.
    pub wake: std::sync::OnceLock<smithay::reexports::calloop::ping::Ping>,
    /// Windows currently mapped in the space.
    pub windows: AtomicU64,
    /// Reserved chords recognised but not acted on — the M4 debt counter.
    pub owed_vt_switches: AtomicU64,
    /// A capture the socket thread has ASKED for, that the render loop has not
    /// yet taken. `Some(path)` means "next frame, write the framebuffer here".
    ///
    /// ★ WHY A REQUEST FIELD AND NOT AN ENV VAR. Capture was env-gated
    /// (`OMOYA_CAPTURE`) while its own comment said the useful moment is "the
    /// seat is already running and something looks wrong". Those contradict:
    /// a process's environment cannot be changed from outside, so the env gate
    /// could only ever be set BEFORE start — precisely not the moment it was
    /// written for. A running seat could never be screenshotted.
    ///
    /// The render loop owns the framebuffer, so it must do the work; the socket
    /// thread only leaves a note. Same direction as every other field here —
    /// the loop pushes, the sidecar never reaches in.
    pub capture_request: std::sync::Mutex<Option<CaptureRequest>>,
    /// Monotonic id handed to each capture/scan request.
    ///
    /// ── ★ WHY A RESULT NEEDS AN IDENTITY ────────────────────────────────
    ///
    /// `capture_result` outlives the client that asked for it: the compositor
    /// is long-lived and the socket client is not. So a FRESH client reading
    /// the leaf gets whatever a PREVIOUS client's request left there, with no
    /// way to tell it apart from its own answer -- a stale success read as a
    /// fresh one, which is the worst shape a diagnostic can take.
    ///
    /// Observed exactly that way: a probe read a `capture_result` written for
    /// a request from an earlier process and reported it as its own.
    ///
    /// The id makes the mismatch VISIBLE rather than removing it -- a caller
    /// still has to compare. That is only-mitigated, not unrepresentable: the
    /// leaf cannot refuse to answer a client that declines to check.
    pub request_seq: std::sync::atomic::AtomicU64,
    /// `frames` at the moment a stale scan was armed.
    ///
    /// ── ★ SO "WAITING" CAN SAY WHICH WAITING IT IS ──────────────────────
    ///
    /// The scan reported `waiting: no frame drawn since the request` and kept
    /// reporting it while the seat was demonstrably compositing --
    /// `last_frame_causes` moved from `chrome` to `commit+deed` under the same
    /// probing. Those are two different situations with the same message:
    ///
    ///   frames unchanged  -> genuinely idle; drive the seat
    ///   frames advanced   -> composites ARE happening and the hook is not
    ///                        being reached, which is a defect in the scan
    ///                        rather than in the seat
    ///
    /// One message for both is the kotae failure this fleet keeps finding: an
    /// answer that cannot distinguish `empty` from `blind`. Recording the frame
    /// count at arm time is the whole cost of telling them apart.
    pub stale_armed_at_frame: std::sync::atomic::AtomicU64,
    /// A pending stale-pixel scan: where to write the mask image.
    ///
    /// ★ Separate from `capture_request` because the two are OPPOSITE
    /// requests. A capture forces `age = 0` — a full repaint — and therefore
    /// REPAIRS the frame it is photographing; a stale scan must run against
    /// the natural age or it destroys its own subject. Sharing one field
    /// would make the more useful of the two impossible to express.
    pub stale_request: std::sync::Mutex<Option<String>>,
    /// The last scan's verdict, as JSON.
    pub stale_result: std::sync::Mutex<Option<String>>,
    /// What became of the last capture: the path written, or the error.
    pub capture_result: std::sync::Mutex<Option<String>>,
    /// ── ★ THE JOINED WINDOW TABLE ────────────────────────────────────────
    ///
    /// One row per toplevel, keyed by a stable id, carrying what the client was
    /// TOLD beside what was actually DRAWN for it.
    ///
    /// It exists because `window_app_ids`, `geometry` and `layout` are three
    /// `" | "`-joined lists with THREE DIFFERENT DENOMINATORS and no join key:
    /// `window_app_ids` walks Space elements (windows only), `geometry` walks
    /// RENDER elements (windows plus the bar plus four focus-ring edges), and
    /// `layout` is structurally EMPTY in floating mode. Nothing could answer
    /// "which window is wrong" -- only "something is".
    ///
    /// ★ The defect class this makes readable is the CONJUNCTION, not any
    /// single value. On plo: `decoration_mode_sent = ServerSide` is a correct
    /// answer, `mode = Floating` is a correct configuration, and
    /// `decoration_elements_drawn = 0` is a correct render -- and together they
    /// mean the operator has no grabbable chrome by any route, which is
    /// precisely the report "the windows have no borders to drag around".
    /// Three legal values, one illegal state, and no leaf could see it.
    pub toplevels: std::sync::Mutex<Vec<ToplevelRow>>,
    /// The bar's height in pixels, so a caller can derive the CONTENT region.
    ///
    /// ── ★ WHY A MASK IS MANDATORY, NOT A NICETY ─────────────────────────
    ///
    /// Measured on plo: three full-frame captures 0.5 s apart are BIT-IDENTICAL,
    /// and across 70 s the ONLY difference is the top 28 rows -- the 1 Hz clock
    /// in the bar. So an unmasked full-frame hash is a clock detector: it
    /// differs every second, for a reason nobody debugging a window cares
    /// about, and a caller learns to ignore it -- which is how a real
    /// regression gets ignored too.
    ///
    /// Published as a NUMBER rather than baked into a `mask: bool` flag so the
    /// caller can see WHY the region starts where it does. A boolean would hide
    /// the derivation, and the derivation is the part worth checking.
    pub bar_height: std::sync::atomic::AtomicU64,
    /// Present intervals in microseconds, bucketed.
    ///
    /// ── ★ WHY A DISTRIBUTION AND NOT A COUNTER ──────────────────────────
    ///
    /// `frames` and `presented` together actively mislead. Measured on plo at
    /// idle: `frames 1190841, presented 384` -- which reads like catastrophic
    /// frame loss and is in fact a pacing loop correctly finding nothing to
    /// draw. Two counters cannot separate "idle" from "starved": both produce
    /// a large ratio.
    ///
    /// A distribution can. An idle seat presents rarely and EVENLY; a starved
    /// one presents in bursts with long gaps. Same ratio, different shape.
    ///
    /// Buckets are fixed and coarse on purpose -- an unbounded histogram of a
    /// 360 Hz seat is a memory leak with a nice name.
    pub present_buckets: [std::sync::atomic::AtomicU64; 6],
    /// Monotonic microseconds at the last presentation, 0 = none yet.
    ///
    /// ★ An atomic rather than a local threaded through the render closure:
    /// the value must survive across loop iterations, and the closure's scope
    /// is not where loop-lifetime state belongs. 0 means "no previous
    /// presentation", which is why the FIRST interval is never bucketed --
    /// bucketing it would record the time since process start as a frame gap.
    pub last_present_us: std::sync::atomic::AtomicU64,
    /// The resolved layout mode, `"tiling"` or `"floating"`.
    ///
    /// ★ Published because the decoration policy's own justification depends on
    /// it: `handlers.rs` answers ServerSide "on a tiling seat the compositor
    /// owns geometry", and plo runs `floating`. A reader could not previously
    /// check that premise from any leaf.
    pub layout_mode: std::sync::Mutex<String>,
    /// What decoration mode each toplevel was TOLD, keyed by a surface id.
    ///
    /// ★ The P side of the table. Recorded where the answer is SENT
    /// (`handlers.rs`'s `XdgDecorationHandler`), not re-derived later, so it
    /// reports what the client actually received rather than what a reader
    /// believes the policy would have said.
    pub decoration_sent: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    /// The input device table: what the backend opened, what it believes
    /// about each one, and whether it is in the poll set.
    ///
    /// ★ EXISTS BECAUSE THE ALTERNATIVE WAS INFERENCE. A keyboard that is
    /// open but silent is indistinguishable from one that is polled and
    /// simply not being typed on — from outside the process there is no way
    /// to tell, and the fds in `/proc/<pid>/fd` look identical either way.
    /// `armed` is the bit that matters: the backend clears it on ENODEV,
    /// which the kernel returns BOTH for an unplug and for a device logind
    /// revoked, and a disarmed device is invisible for the rest of the run.
    pub input_devices: std::sync::Mutex<String>,
    /// Every mode the connector offers, and which one we took —
    /// `"1920x1080@60 1920x1080@360* …"`, the star marking the selection.
    ///
    /// Exists because "we are running at 60 Hz" and "60 Hz is all the panel
    /// offers" are indistinguishable from outside, and the difference is the
    /// whole bug: plo's 360 Hz display advertises six modes at its native
    /// resolution and EDID flags the 60 Hz one as PREFERRED.
    pub modes: std::sync::Mutex<String>,
    /// Why the last frame happened — `"commit"`, `"pointer+chrome"`, … .
    ///
    /// The answer to "it is repainting, but nothing is moving — what is
    /// dirtying the screen?", which is otherwise unanswerable from outside
    /// the process. Names rather than a bitmask because a number would need
    /// this file open to read.
    pub last_frame_causes: std::sync::Mutex<String>,
    /// The handle for telling the render loop a frame is owed.
    ///
    /// ★ **The socket thread MUST mark whenever it leaves work for the
    /// loop.** Since the loop became damage-driven, waking it is no longer
    /// enough: an un-marked wake finds nothing owed, skips, and the request
    /// sits in its mutex until something unrelated dirties the screen. A
    /// queued deed that never runs looks exactly like a deed that ran and did
    /// nothing, which is the failure this note exists to prevent.
    ///
    /// A `OnceLock` rather than a plain field because `OmoyaIntrospect` is
    /// built (and `Arc`-shared) before the gate exists, and because `OnceLock`
    /// is `Default` — so the sidecar's derive keeps working untouched.
    /// [`Self::mark`] treats "not yet set" as a no-op, which is correct: no
    /// loop is running to owe a frame to.
    pub owed: std::sync::OnceLock<mekuri::Ledger<crate::owed::Owed>>,
    /// Scanout width/height, 0 when nested or not yet known.
    pub output_w: AtomicU64,
    pub output_h: AtomicU64,
    /// Refresh in Hz — the rate the seat is actually PACING at.
    ///
    /// ★ THIS LEAF ALREADY EXISTED AND WOULD HAVE NAMED THE BUG. On plo it
    /// read the panel's `vrefresh`, which is an OPTIONAL DRM field that this
    /// panel leaves at 0 — and `frame_interval`'s `.max(1)` turned that into
    /// a 1 Hz desktop. The number was queryable the whole time; nobody asked
    /// it, because every other signal was healthy and the only symptom was a
    /// human saying the machine felt slow.
    ///
    /// It now carries the DERIVED rate (`refresh_hz()` in `drm.rs`), so it
    /// reports what the loop is really doing rather than what the driver
    /// declined to say. A leaf that reports an unset field is a leaf that
    /// looks like it is answering.
    pub refresh_hz: AtomicU64,
    /// Whether libinput attached. An agent debugging "I cannot type" should be
    /// able to ask this rather than infer it from a log line.
    pub input_attached: AtomicU64,
    /// Whether logind/libseat currently considers this session active.
    ///
    /// ★ 1 while the seat is ours, 0 while another VT holds it. Published
    /// because "why is the screen not updating" and "another VT has the seat"
    /// are indistinguishable from outside — and this is the surface that tells
    /// them apart without someone walking to the machine.
    pub session_active: AtomicU64,
    /// Session activate/pause events observed since start.
    ///
    /// ★ Zero after a VT switch means the notifier is not wired — the exact
    /// defect this counter was added alongside. A count that never moves is
    /// the symptom; before the fix it could not have moved at all.
    pub session_events: AtomicU64,
    /// The Wayland socket clients connect on.
    pub socket: std::sync::OnceLock<String>,
    /// Seat mode, as `SeatMode::name()` spells it.
    pub mode: std::sync::OnceLock<String>,
}

impl OmoyaIntrospect {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Queue one synthetic input action and wake the loop.
    ///
    /// ★ Marks AND pings, both required and not the same thing — the ping
    /// makes the loop run, the mark makes that run do something. Since the
    /// loop became damage-driven a bare ping finds nothing owed and goes
    /// straight back to sleep with the input still queued.
    fn queue_input(&self, s: crate::synth::Synth) {
        self.pending_input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(s);
        self.mark(crate::owed::Owed::Deed);
        if let Some(p) = self.wake.get() {
            p.ping();
        }
    }

    /// Tell the render loop a frame is owed, from any thread.
    ///
    /// A no-op before the gate is installed — nothing is rendering yet, so
    /// there is no frame to owe.
    pub fn mark(&self, cause: crate::owed::Owed) {
        if let Some(l) = self.owed.get() {
            l.mark(cause);
        }
    }

    /// Publish the per-tick facts. Called from the render loop, so it must stay
    /// allocation-free and lock-free — this runs once per frame.
    pub fn tick(&self, windows: u64) {
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.windows.store(windows, Ordering::Relaxed);
    }
}

impl Introspect for OmoyaIntrospect {
    fn query(&self, q: &Query) -> QueryResult {
        // A hand-written impl rather than `#[derive(Introspect)]`, matching what
        // every existing consumer does: the derive handles named struct fields,
        // and half of what is interesting here is a computed or `OnceLock`
        // value.
        let head = q.path.first().map(String::as_str).unwrap_or_default();
        let g = |v: &std::sync::OnceLock<String>| {
            serde_json::json!(v.get().cloned().unwrap_or_default())
        };
        let n = |a: &AtomicU64| serde_json::json!(a.load(Ordering::Relaxed));

        match head {
            "backend" => Ok(serde_json::json!(
                if self.backend.load(Ordering::Relaxed) == 1 {
                    "drm"
                } else {
                    "nested"
                }
            )),
            // ★ `unknown` is its own arm, not a default to one of the two.
            // A seat that has not modeset yet genuinely does not know, and
            // reporting a guess here would be worse than reporting nothing:
            // the whole point of the leaf is to settle which path is live.
            "atomic" => Ok(serde_json::json!(
                match self.atomic.load(Ordering::Relaxed) {
                    1 => "atomic",
                    2 => "legacy",
                    _ => "unknown",
                }
            )),
            "frames" => Ok(n(&self.frames)),
            "presented" => Ok(n(&self.presented)),
            // ── ★ THE ONE MUTATING LEAF ──────────────────────────────
            //
            // `["do", "<verb>"]`. Every other leaf on this socket is a read;
            // this is the write surface, and it is deliberately ONE leaf with
            // a closed vocabulary rather than a family of them. `Deed::parse`
            // is the legality gate — an unknown verb has no Deed and
            // therefore no path to `perform`.
            //
            // The answer distinguishes the four kotae cases: a queued verb is
            // `found`, an unknown verb is `refused` BY NAME, and a missing
            // argument is refused as such rather than defaulting.
            "do" => {
                // `unknown_field`, not a bespoke variant — kanshou's error
                // vocabulary is closed (UnknownField / UnknownMethod /
                // TypeMismatch / BadArity / Internal) and there is no
                // NotFound. Reaching for one that does not exist is how a
                // refusal ends up rendered as an internal error, which reads
                // to the caller as "the seat is broken" rather than "you
                // asked for something that is not a verb".
                let Some(verb) = q.path.get(1) else {
                    return Err(QueryError::unknown_field(
                        "do needs a verb, e.g. do/focus-right — \
                         list them with `verbs`",
                    ));
                };
                let Some(deed) = crate::deed::Deed::parse(verb) else {
                    return Err(QueryError::unknown_field(format!(
                        "{verb:?} is not a verb this seat accepts; \
                         the accepted set is `verbs`"
                    )));
                };
                self.pending_deeds
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(deed);
                // Owe a frame, THEN wake the loop. Both are required and they
                // are not the same thing: the ping makes the loop run a tick,
                // and the mark makes that tick do something. Since the loop
                // became damage-driven a bare ping wakes it to find nothing
                // owed, and it goes straight back to sleep with the deed
                // still queued.
                self.mark(crate::owed::Owed::Deed);
                if let Some(p) = self.wake.get() {
                    p.ping();
                }
                Ok(serde_json::json!(format!("queued: {verb}")))
            }
            // ── ★ THE INPUT WRITE SURFACE ───────────────────────────
            //
            // Queued, then drained by the ping source where `&mut Omoya` is
            // legal — the same seam `do` uses, for the same reason. The reply
            // says "queued" and NOTHING about whether it landed; read
            // `synth_performed` for that, which the compositor thread writes.
            "type" => {
                let Some(text) = q.args.first().and_then(|v| v.as_str()) else {
                    return Err(QueryError::unknown_field(
                        "type needs a string argument, e.g. type \"ls -la\\n\"",
                    ));
                };
                let synth = crate::synth::Synth::Text(text.to_string());
                // ★ VALIDATED BEFORE QUEUEING. `expand` is where an
                // unmappable character is refused; doing it here means the
                // caller is told, instead of the render thread silently
                // dropping half a string an hour later.
                let steps =
                    crate::synth::expand(&synth).map_err(|e| QueryError::unknown_field(e))?;
                let n = steps.len();
                self.queue_input(synth);
                Ok(serde_json::json!({ "queued": text, "steps": n }))
            }
            "key" => {
                let Some(code) = q.args.first().and_then(serde_json::Value::as_u64) else {
                    return Err(QueryError::unknown_field(
                        "key needs an evdev keycode, e.g. key 28 (KEY_ENTER)",
                    ));
                };
                let down = q.args.get(1).and_then(serde_json::Value::as_bool);
                let code = u32::try_from(code)
                    .map_err(|_| QueryError::unknown_field("keycode out of range"))?;
                match down {
                    // No explicit state: a tap, so a caller never has to
                    // remember to release and cannot strand a modifier.
                    None => {
                        self.queue_input(crate::synth::Synth::Key {
                            code,
                            pressed: true,
                        });
                        self.queue_input(crate::synth::Synth::Key {
                            code,
                            pressed: false,
                        });
                        Ok(serde_json::json!({ "queued": "tap", "code": code }))
                    }
                    Some(pressed) => {
                        self.queue_input(crate::synth::Synth::Key { code, pressed });
                        Ok(
                            serde_json::json!({ "queued": "hold", "code": code, "pressed": pressed }),
                        )
                    }
                }
            }
            "pointer" => {
                let (Some(dx), Some(dy)) = (
                    q.args.first().and_then(serde_json::Value::as_f64),
                    q.args.get(1).and_then(serde_json::Value::as_f64),
                ) else {
                    return Err(QueryError::unknown_field(
                        "pointer needs dx and dy, e.g. pointer -40 20",
                    ));
                };
                self.queue_input(crate::synth::Synth::Pointer { dx, dy });
                Ok(serde_json::json!({ "queued": "pointer", "dx": dx, "dy": dy }))
            }
            "click" => {
                // BTN_LEFT unless told otherwise; press and release, so a
                // caller cannot leave a button down on the operator's desktop.
                let code = q
                    .args
                    .first()
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok())
                    .unwrap_or(272);
                self.queue_input(crate::synth::Synth::Button {
                    code,
                    pressed: true,
                });
                self.queue_input(crate::synth::Synth::Button {
                    code,
                    pressed: false,
                });
                Ok(serde_json::json!({ "queued": "click", "code": code }))
            }
            "synth_performed" => Ok(n(&self.synth_performed)),
            "verbs" => Ok(serde_json::json!(crate::deed::Deed::VERBS)),
            "deeds_performed" => Ok(n(&self.deeds_performed)),
            "chord_deeds" => Ok(n(&self.chord_deeds)),
            "focus_rect" => Ok(serde_json::json!(
                self.focus_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .map_or_else(
                        || "none".to_string(),
                        |(x, y, w, h)| format!("{x},{y} {w}x{h}")
                    )
            )),
            "frame_us" => Ok(n(&self.frame_us)),
            "blit_fast" => Ok(n(&self.blit_fast)),
            "blit_slow" => Ok(n(&self.blit_slow)),
            "blit_general" => Ok(n(&self.blit_general)),
            "route_cpu_bytes" => Ok(serde_json::json!(
                self.route_cpu_bytes
                    .load(std::sync::atomic::Ordering::Relaxed)
            )),
            "route_cpu_bytes_total" => Ok(serde_json::json!(
                self.route_cpu_bytes_total
                    .load(std::sync::atomic::Ordering::Relaxed)
            )),
            "planes" => Ok(serde_json::json!(
                *self.planes.lock().unwrap_or_else(|e| e.into_inner())
            )),
            "route_label" => Ok(serde_json::json!(
                *self.route_label.lock().unwrap_or_else(|e| e.into_inner())
            )),
            "shm_imports" => Ok(serde_json::json!(
                self.shm_imports.load(std::sync::atomic::Ordering::Relaxed)
            )),
            "shm_imports_empty_damage" => Ok(serde_json::json!(
                self.shm_imports_empty_damage
                    .load(std::sync::atomic::Ordering::Relaxed)
            )),
            "shm_damage_rects" => Ok(serde_json::json!(
                self.shm_damage_rects
                    .load(std::sync::atomic::Ordering::Relaxed)
            )),
            "shm_damage_area" => Ok(serde_json::json!(
                self.shm_damage_area
                    .load(std::sync::atomic::Ordering::Relaxed)
            )),
            "gather_us" => Ok(n(&self.gather_us)),
            "flush_us" => Ok(n(&self.flush_us)),
            "flush_us_max" => Ok(n(&self.flush_us_max)),
            "flush_us_total" => Ok(n(&self.flush_us_total)),
            "flush_bytes" => Ok(n(&self.flush_bytes)),
            "flush_bytes_total" => Ok(n(&self.flush_bytes_total)),
            // The derived reading, computed here so two callers cannot derive
            // it differently. MB/s over the whole run: bytes_total / us_total.
            // A figure far below the machine's memcpy capability means the
            // flush is contending, not working.
            "flush_mb_per_s" => {
                let us = self.flush_us_total.load(std::sync::atomic::Ordering::Relaxed);
                let by = self.flush_bytes_total.load(std::sync::atomic::Ordering::Relaxed);
                Ok(if us == 0 {
                    serde_json::json!(null)
                } else {
                    serde_json::json!((by as f64) / (us as f64))
                })
            }
            // ★ WHICH MODE IS LIVE. Reading `td_dirty_pct` without this is a
            // measurement with no idea whether it changed anything: `verify`
            // publishes identical counters while leaving the screen untouched.
            "td_mode" => Ok(serde_json::json!(match self
                .td_mode
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                0 => "off",
                2 => "verify",
                _ => "on",
            })),
            // ★ THE LIVE KNOB. Flipping the mode without a rebuild or a
            // relogin is what makes an A/B possible AT ALL on a seat whose
            // console is the machine's only local access — the alternative is
            // a rebuild-and-log-back-in cycle per experiment, which nobody
            // runs twice, so the question stays open instead.
            "td_mode_set" => {
                let Some(name) = q.args.first().and_then(serde_json::Value::as_str) else {
                    return Err(QueryError::unknown_field(
                        "td_mode_set needs a mode: off | on | verify",
                    ));
                };
                match crate::truedamage::Mode::parse(name) {
                    Ok(m) => {
                        let was = self
                            .td_mode
                            .swap(m.to_u64(), std::sync::atomic::Ordering::Relaxed);
                        Ok(serde_json::json!({
                            "was": crate::truedamage::Mode::from_u64(was).name(),
                            "now": m.name(),
                        }))
                    }
                    Err(e) => Err(QueryError::unknown_field(e)),
                }
            }
            // ★ WHO IS ON THE SEAT, BY THE NAME THE RULES MATCH ON.
            //
            // Added because the floating rule silently did not fire and there
            // was no way to ask why: `layout` publishes the tiled tree and
            // `geometry` publishes rectangles, and neither says which window
            // is which. A placement rule keyed on `app_id` that cannot be
            // asked "what app_id did you see?" is a rule that can only be
            // debugged by rebuilding the compositor.
            //
            // `null` for a window that has not set one — distinct from the
            // empty string, which is a client that set one and chose nothing.
            // ★ The joined table. Supersedes window_app_ids/geometry/layout,
            // which stay for now as legacy projections -- coexistence is
            // only-mitigated; re-deriving them FROM this table is what would
            // make a disagreement unrepresentable, and that is a later step.
            "toplevels" => {
                let floating = self
                    .layout_mode
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_str()
                    == "floating";
                let rows = self.toplevels.lock().unwrap_or_else(|e| e.into_inner());
                Ok(serde_json::json!({
                    "count": rows.len(),
                    "floating": floating,
                    "rows": rows.iter().map(|r| {
                        let mut v = serde_json::to_value(r).unwrap_or(serde_json::Value::Null);
                        if let Some(o) = v.as_object_mut() {
                            o.insert("chrome_verdict".into(),
                                serde_json::Value::String(r.chrome_verdict(floating).to_owned()));
                        }
                        v
                    }).collect::<Vec<_>>(),
                }))
            }
            "bar_height" => Ok(n(&self.bar_height)),
            "present_intervals" => {
                // Bucket edges in microseconds. 2778us is one frame at 360Hz,
                // so the first bucket is "kept up" and the last is "a human
                // would call that a freeze".
                const EDGES: [&str; 6] = [
                    "<=2.8ms (360Hz)",
                    "<=8.3ms (120Hz)",
                    "<=16.7ms (60Hz)",
                    "<=50ms",
                    "<=250ms",
                    ">250ms",
                ];
                let counts: Vec<u64> = self
                    .present_buckets
                    .iter()
                    .map(|b| b.load(std::sync::atomic::Ordering::Relaxed))
                    .collect();
                let total: u64 = counts.iter().sum();
                Ok(serde_json::json!({
                    "total": total,
                    "buckets": EDGES.iter().zip(&counts)
                        .map(|(e, c)| serde_json::json!({ "upto": e, "count": c }))
                        .collect::<Vec<_>>(),
                    "note": "an idle seat presents rarely and EVENLY; a starved \
                             one presents in bursts. frames/presented cannot \
                             tell those apart -- this can.",
                }))
            }
            "layout_mode" => Ok(serde_json::json!(
                self.layout_mode
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            )),
            "window_app_ids" => Ok(serde_json::json!(
                self.window_app_ids
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            )),
            "td_refined" => Ok(n(&self.td_refined)),
            "td_refused" => Ok(n(&self.td_refused)),
            "td_rows_dirty" => Ok(n(&self.td_rows_dirty)),
            "td_rows_examined" => Ok(n(&self.td_rows_examined)),
            "td_shadows" => Ok(n(&self.td_shadows)),
            "td_presented_marks" => Ok(n(&self.td_presented_marks)),
            // The one number an operator actually wants: what fraction of the
            // surface a commit really changes. Derived here rather than left
            // to the caller so the denominator cannot be dropped on the way.
            "td_dirty_pct" => {
                let ex = self
                    .td_rows_examined
                    .load(std::sync::atomic::Ordering::Relaxed);
                let di = self
                    .td_rows_dirty
                    .load(std::sync::atomic::Ordering::Relaxed);
                Ok(if ex == 0 {
                    // ★ NOT 0.0. Zero examined means the refinement never ran,
                    // which would render as "0% dirty" — a perfect score for a
                    // broken feature.
                    serde_json::json!("no rows examined")
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    serde_json::json!(format!("{:.2}%", 100.0 * di as f64 / ex as f64))
                })
            }
            "import_full" => Ok(n(&self.import_full)),
            "import_partial" => Ok(n(&self.import_partial)),
            "elements" => Ok(n(&self.elements)),
            "geometry" => Ok(serde_json::json!(
                self.geometry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .join(" | ")
            )),
            "layout" => Ok(serde_json::json!(
                self.layout
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .join(" | ")
            )),
            "windows" => Ok(n(&self.windows)),
            "owed_vt_switches" => Ok(n(&self.owed_vt_switches)),
            // Why the LAST frame happened. Sourced from the drained mask the
            // render loop actually acted on, not from a re-read of the
            // ledger — a re-read would be a different question (what is owed
            // NOW) wearing this one's name.
            "input_devices" => Ok(serde_json::json!(
                self.input_devices
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            )),
            "modes" => Ok(serde_json::json!(
                self.modes.lock().unwrap_or_else(|e| e.into_inner()).clone()
            )),
            "last_frame_causes" => Ok(serde_json::json!(
                self.last_frame_causes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            )),
            // Whether a frame is owed RIGHT NOW. Diagnostic: on a healthy
            // idle seat this is `false` and `presented` is not climbing.
            // `true` while `presented` stays put means the loop is not
            // running — a wedged tick, not a missing mark.
            "owed" => Ok(serde_json::json!(
                self.owed.get().is_some_and(mekuri::Ledger::peek_owed)
            )),
            // The closed cause vocabulary, so a caller can read
            // `last_frame_causes` without this file open.
            "owed_causes" => Ok(serde_json::json!(
                <crate::owed::Owed as mekuri::Cause>::all()
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
            )),
            // Ask for a screenshot. `capture` with a path argument leaves the
            // request; the render loop takes it on its next frame. Returns
            // immediately with "requested" — the caller then reads
            // `capture_result`, which is how an async job stays honest about
            // not having finished yet.
            "capture" => {
                let Some(path) = q.args.first().and_then(|v| v.as_str()) else {
                    return Err(QueryError::UnknownField {
                        field: "capture needs a path argument".to_string(),
                    });
                };
                // Optional args after the path: x y w h (region), then a
                // literal "hash" to hash instead of writing pixels.
                let nums: Vec<i32> = q
                    .args
                    .iter()
                    .skip(1)
                    .filter_map(|v| v.as_i64().and_then(|n| i32::try_from(n).ok()))
                    .collect();
                let region = if nums.len() >= 4 {
                    Some((nums[0], nums[1], nums[2], nums[3]))
                } else {
                    None
                };
                let hash_only = q
                    .args
                    .iter()
                    .any(|v| v.as_str().is_some_and(|s| s == "hash"));
                let id = self
                    .request_seq
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                *self
                    .capture_request
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(CaptureRequest {
                    id,
                    path: path.to_string(),
                    region,
                    hash_only,
                });
                // ★ Cleared so a caller cannot read the PREVIOUS request's
                // result and believe it is this one's. The id below is what
                // makes that check possible at all; clearing alone is a race.
                *self
                    .capture_result
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                // Same pairing as `do` above: owe the frame, then wake. A
                // screenshot of an idle seat is precisely the case where no
                // client is going to commit on our behalf.
                self.mark(crate::owed::Owed::Capture);
                if let Some(p) = self.wake.get() {
                    p.ping();
                }
                Ok(serde_json::json!({
                    "requested": path,
                    "request_id": id,
                    "region": region.map(|r| vec![r.0, r.1, r.2, r.3]),
                    "hash_only": hash_only,
                    "note": "compare capture_result.request_id against this",
                }))
            }
            // ── ★ THE TOOL THAT SEES WHAT A SCREENSHOT CANNOT ────────
            // `capture` reads the shadow, and asking for one forces a full
            // repaint. Both together mean a screenshot shows what the
            // compositor BELIEVES and repairs the frame in the act of
            // asking — measured 2026-08-28, two captures across a
            // deliberate stale-pixel hunt came back byte-identical.
            //
            // This compares the shadow against the SCANOUT MAPPING at the
            // natural age, which is the only place the difference exists.
            // Read `stale_result` for the verdict.
            "stale_scan" => {
                let path = q
                    .args
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("/tmp/omoya-stale.ppm");
                *self.stale_request.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(path.to_string());
                *self.stale_result.lock().unwrap_or_else(|e| e.into_inner()) = None;
                // Armed against the SAME counter the wait is measured in;
                // two different counters here is how the bug above was built.
                self.stale_armed_at_frame.store(
                    self.presented.load(std::sync::atomic::Ordering::Relaxed),
                    std::sync::atomic::Ordering::Relaxed,
                );
                // ★ NOT `Owed::Capture`, and this is the whole subtlety: a
                // scan must observe a frame the seat was going to draw
                // ANYWAY. Owing one would compose a fresh frame whose damage
                // is by definition complete, and the scan would report a
                // clean seat on a broken one. So it waits, and on a fully
                // idle seat it reports `waiting` rather than a false pass.
                if let Some(p) = self.wake.get() {
                    p.ping();
                }
                Ok(serde_json::json!({ "requested": path, "note":
                    "runs on the next naturally-drawn frame; read stale_result" }))
            }
            "stale_result" => {
                let pending = self
                    .stale_request
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some();
                let done = self
                    .stale_result
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(v) = done {
                    return Ok(serde_json::json!(v));
                }
                if !pending {
                    return Ok(serde_json::json!("no scan has been requested"));
                }
                // ★ `presented`, NOT `frames`. `frames` counts TICKS OF THE
                // EVENT LOOP (drm.rs:861 says so in as many words), and the
                // loop ticks on a timer whether or not anything composites. So
                // reading `frames` here made an IDLE SEAT — the overwhelmingly
                // common case — report "composites ARE happening and the scan
                // hook was not reached", sending the reader to debug a scan
                // that was working perfectly.
                //
                // Measured 2026-08-30: `frames` advanced 2946 while `presented`
                // sat at 604 across four seconds. Exactly the kotae failure
                // this block's own comment claims to have fixed — two
                // different waits, named, but keyed off the wrong counter.
                let since = self
                    .presented
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .saturating_sub(
                        self.stale_armed_at_frame
                            .load(std::sync::atomic::Ordering::Relaxed),
                    );
                // ★ TWO DIFFERENT WAITS, NAMED. Same message for both is the
                // kotae failure: `empty` and `blind` rendered identically.
                Ok(serde_json::json!({
                    "outcome": "waiting",
                    "frames_since_armed": since,
                    "verdict": if since == 0 {
                        "the seat has composited NOTHING since arming — it is \
                         idle. Drive it (move the pointer, type) to produce a \
                         naturally-drawn frame; the scan is not at fault"
                    } else {
                        "composites ARE happening and the scan hook was not \
                         reached — this is a defect in the scan, not the seat"
                    },
                }))
            }
            "stale_result_raw" => Ok(serde_json::json!(
                self.stale_result
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or_else(|| "waiting: no frame drawn since the request".into())
            )),
            "capture_result" => Ok(serde_json::json!(
                self.capture_result
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            )),
            "input_attached" => Ok(serde_json::json!(
                self.input_attached.load(Ordering::Relaxed) == 1
            )),
            "session_active" => Ok(serde_json::json!(
                self.session_active.load(Ordering::Relaxed) == 1
            )),
            "session_events" => Ok(n(&self.session_events)),
            "socket" => Ok(g(&self.socket)),
            "mode" => Ok(g(&self.mode)),
            "output" => Ok(serde_json::json!({
                "width": self.output_w.load(Ordering::Relaxed),
                "height": self.output_h.load(Ordering::Relaxed),
                "refresh_hz": self.refresh_hz.load(Ordering::Relaxed),
            })),
            // ★ One leaf that answers the question this session kept asking a
            // human: is the seat actually drawing, and on what.
            "seat" => Ok(serde_json::json!({
                "backend": if self.backend.load(Ordering::Relaxed) == 1 { "drm" } else { "nested" },
                "mode": self.mode.get().cloned().unwrap_or_default(),
                "socket": self.socket.get().cloned().unwrap_or_default(),
                "frames": self.frames.load(Ordering::Relaxed),
                "presented": self.presented.load(Ordering::Relaxed),
                "deeds_performed": self.deeds_performed.load(Ordering::Relaxed),
                "elements": self.elements.load(Ordering::Relaxed),
                "geometry": self.geometry.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                "layout": self.layout.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                "windows": self.windows.load(Ordering::Relaxed),
                "input_attached": self.input_attached.load(Ordering::Relaxed) == 1,
                "output": {
                    "width": self.output_w.load(Ordering::Relaxed),
                    "height": self.output_h.load(Ordering::Relaxed),
                    "refresh_hz": self.refresh_hz.load(Ordering::Relaxed),
                },
            })),
            other => Err(QueryError::UnknownField {
                field: other.to_string(),
            }),
        }
    }

    fn schema(&self) -> &'static [&'static str] {
        &[
            "backend",
            "frames",
            "presented",
            "verbs",
            "deeds_performed",
            "chord_deeds",
            "focus_rect",
            "frame_us",
            "planes",
            "route_cpu_bytes",
            "route_cpu_bytes_total",
            "route_label",
            "shm_imports",
            "shm_imports_empty_damage",
            "shm_damage_rects",
            "shm_damage_area",
            "blit_fast",
            "blit_general",
            "blit_slow",
            "gather_us",
            "flush_bytes",
            "flush_bytes_total",
            "flush_mb_per_s",
            "flush_us",
            "flush_us_max",
            "flush_us_total",
            "window_app_ids",
            "td_mode",
            "td_refined",
            "td_refused",
            "td_rows_dirty",
            "td_rows_examined",
            "td_shadows",
            "td_presented_marks",
            "td_dirty_pct",
            "import_full",
            "import_partial",
            "elements",
            "geometry",
            "layout",
            "windows",
            "owed_vt_switches",
            "capture_result",
            "stale_result",
            "stale_result_raw",
            "toplevels",
            "layout_mode",
            "bar_height",
            "present_intervals",
            "atomic",
            // ★ THESE WERE ANSWERED AND UNLISTED. `schema()` is how an agent
            // discovers what it can ask, so a leaf missing here is a leaf that
            // effectively does not exist — and `last_frame_causes` is the one
            // that named mado as the idle-repaint source in a single query
            // after the compositor had been suspected for hours. The
            // every-schema-leaf-answers gate is one-directional and could not
            // catch this; the reverse gate below now can.
            "last_frame_causes",
            "owed",
            "owed_causes",
            "modes",
            "input_devices",
            "synth_performed",
            "input_attached",
            "session_active",
            "session_events",
            "socket",
            "mode",
            "output",
            "seat",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_schema_leaf_answers() {
        // ★ The gate that keeps `schema()` honest. A leaf advertised but not
        // handled is worse than an absent one: an agent asks for it, gets
        // `unknown-field`, and cannot tell a typo from a lie.
        let s = OmoyaIntrospect::default();
        for leaf in s.schema() {
            let q = Query::field(vec![(*leaf).to_string()]);
            assert!(
                s.query(&q).is_ok(),
                "schema advertises `{leaf}` but query() does not answer it"
            );
        }
    }

    #[test]
    fn every_answered_leaf_is_advertised() {
        // ★ THE REVERSE GATE, AND THE ONE THAT WAS MISSING.
        //
        // `every_schema_leaf_answers` catches a leaf advertised and not
        // handled. Nothing caught the opposite — a leaf HANDLED and not
        // advertised — and that is the direction that actually bit: an agent
        // reads `schema()` to learn what it may ask, so an unlisted leaf
        // effectively does not exist. `last_frame_causes` sat unlisted while
        // being the leaf that identified mado as the idle-repaint source in
        // one query, after the compositor had been suspected for hours.
        //
        // Enumerated from the source rather than from a second hand-list,
        // because a hand-list would need updating in the same commit as the
        // match arm — which is exactly the discipline that already failed.
        let src = include_str!("introspect.rs");
        let body = src
            .split_once("fn query(")
            .map(|(_, rest)| rest)
            .unwrap_or(src);
        let mut answered: Vec<String> = Vec::new();
        for line in body.lines() {
            let t = line.trim();
            // Match arms of the form `"name" => ...`, which is how every leaf
            // is written. Methods that take arguments (`do`, `type`, `key`,
            // `pointer`, `click`, `capture`, `td_mode_set`) are excluded: they
            // are verbs, not readable fields, and `schema()` advertises fields.
            // Every one of them has a READ counterpart that IS advertised —
            // `td_mode_set` writes what `td_mode` reads back — so excluding
            // the verb never hides state from an agent enumerating the seat.
            let Some(rest) = t.strip_prefix('"') else {
                continue;
            };
            let Some((name, after)) = rest.split_once('"') else {
                continue;
            };
            if !after.trim_start().starts_with("=>") {
                continue;
            }
            const VERBS: &[&str] = &[
                "do",
                "type",
                "key",
                "pointer",
                "click",
                "capture",
                // A diagnostic REQUEST, not a field: it schedules a scan on
                // the next naturally-drawn frame. `stale_result` reads the
                // verdict back, and IS advertised as a leaf.
                "stale_scan",
                // A live-tuning SETTER, not a field. `td_mode` reads it back.
                "td_mode_set",
            ];
            if VERBS.contains(&name) || name.is_empty() {
                continue;
            }
            answered.push(name.to_string());
        }
        assert!(
            answered.len() > 20,
            "the arm scan found only {} leaves — it has stopped matching the \
             source shape and is now a vacuous gate",
            answered.len()
        );
        let s = OmoyaIntrospect::default();
        let advertised: Vec<&str> = s.schema().to_vec();
        let missing: Vec<&String> = answered
            .iter()
            .filter(|a| !advertised.contains(&a.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "these leaves are answered but not advertised in schema(), so no \
             caller can discover them: {missing:?}"
        );
    }

    #[test]
    fn an_unknown_leaf_is_refused_by_name() {
        let s = OmoyaIntrospect::default();
        match s.query(&Query::field(vec!["nonesuch"])) {
            Err(QueryError::UnknownField { field }) => assert_eq!(field, "nonesuch"),
            other => panic!("expected UnknownField, got {other:?}"),
        }
    }

    #[test]
    fn the_seat_leaf_reports_what_it_was_told() {
        let s = OmoyaIntrospect::default();
        s.backend.store(1, Ordering::Relaxed);
        s.input_attached.store(1, Ordering::Relaxed);
        s.output_w.store(1024, Ordering::Relaxed);
        s.output_h.store(768, Ordering::Relaxed);
        s.tick(3);

        let v = s.query(&Query::field(vec!["seat"])).unwrap();
        assert_eq!(v["backend"], "drm");
        assert_eq!(v["input_attached"], true);
        assert_eq!(v["output"]["width"], 1024);
        assert_eq!(v["windows"], 3);
        assert_eq!(v["frames"], 1);
    }
}

impl OmoyaIntrospect {
    /// Publish the true-damage counters after a commit.
    ///
    /// Takes the whole `Shadows` rather than individual numbers so the counts
    /// and their denominator can only ever be published together — a caller
    /// cannot accidentally ship `rows_dirty` without `rows_examined`, which is
    /// the shape that makes a broken refinement read as a perfect one.
    pub fn publish_truedamage(
        &self,
        shadows: &crate::truedamage::Shadows,
        _verdict: &crate::truedamage::Verdict,
    ) {
        use std::sync::atomic::Ordering::Relaxed;
        self.td_refined.store(shadows.refined, Relaxed);
        self.td_refused.store(shadows.refused, Relaxed);
        self.td_rows_dirty.store(shadows.rows_dirty, Relaxed);
        self.td_rows_examined.store(shadows.rows_examined, Relaxed);
        self.td_shadows.store(shadows.len() as u64, Relaxed);
        self.td_presented_marks
            .store(shadows.presented_marks, Relaxed);
    }
}

#[cfg(test)]
mod toplevel_table_tests {
    use super::ToplevelRow;

    fn row(sent: Option<&str>, drawn: u32) -> ToplevelRow {
        ToplevelRow {
            id: 0,
            app_id: Some("mado".into()),
            decoration_mode_sent: sent.map(ToOwned::to_owned),
            rect: Some((518, 280, 883, 547)),
            decoration_elements_drawn: drawn,
            focused: true,
            tiled: false,
        }
    }

    /// ★ THE OPERATOR'S REPORT, AS A TEST (plo, 2026-08-29).
    ///
    /// "the windows have no borders for me to drag around and such" / "or snap".
    /// Every input below is individually correct: omoya answers ServerSide by
    /// deliberate policy, the seat is configured floating, and the renderer
    /// draws a focus ring only for the focused window. The CONJUNCTION is the
    /// defect, and no per-field leaf could show it -- which is exactly why the
    /// three legacy lists could not answer "which window is wrong".
    #[test]
    fn the_operators_report_is_a_named_verdict() {
        let v = row(Some("ServerSide"), 0).chrome_verdict(true);
        assert!(v.starts_with("no-grabbable-chrome"), "{v}");
        assert!(v.contains("ServerSide") && v.contains("floating"), "{v}");
    }

    /// Anti-vacuity: the verdict must DISCRIMINATE, not always accuse.
    /// A constant "no-grabbable-chrome" would pass the test above.
    #[test]
    fn a_healthy_window_is_not_accused() {
        assert_eq!(
            row(Some("ServerSide"), 4).chrome_verdict(true),
            "server-side drawn"
        );
        assert_eq!(
            row(Some("ClientSide"), 0).chrome_verdict(true),
            "client-side (client draws its own)",
            "a client drawing its own titlebar has chrome even at 0 server elements"
        );
    }

    /// Tiling is the case the ServerSide policy was WRITTEN for, and there the
    /// same promise-with-nothing-drawn is a milder statement -- the compositor
    /// owns geometry, so the operator still has keyboard control.
    #[test]
    fn tiling_and_floating_are_not_the_same_verdict() {
        assert_ne!(
            row(Some("ServerSide"), 0).chrome_verdict(true),
            row(Some("ServerSide"), 0).chrome_verdict(false),
            "the layout mode must change the verdict -- it is the premise the \
             decoration policy cites for itself"
        );
    }

    /// `None` is not `ServerSide`. A default would collapse "not told yet" into
    /// "told server-side", turning a startup race into a policy report.
    #[test]
    fn not_yet_told_is_its_own_answer() {
        assert_eq!(
            row(None, 0).chrome_verdict(true),
            "client-side (client draws its own)",
            "absence must not be read as a ServerSide promise"
        );
    }
}

#[cfg(test)]
mod capture_identity_tests {
    use super::*;

    /// ★ THE STALE-SUCCESS READ (observed 2026-08-29).
    ///
    /// `capture_result` outlives the client that asked for it: the compositor
    /// is long-lived, a socket client is not. A fresh client reading the leaf
    /// gets whatever a PREVIOUS client's request left, and without an identity
    /// on the answer it cannot tell that from its own — a stale success read
    /// as fresh, which is the worst shape a diagnostic can take.
    #[test]
    fn every_request_gets_a_distinct_id() {
        let i = OmoyaIntrospect::default();
        let ids: Vec<u64> = (0..5)
            .map(|_| {
                i.request_seq
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1
            })
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "ids must not repeat: {ids:?}");
        assert!(
            ids.windows(2).all(|w| w[1] > w[0]),
            "must increase: {ids:?}"
        );
    }

    /// The id must start above zero, so "no request yet" (0 / absent) is never
    /// confusable with "request number zero".
    #[test]
    fn the_first_id_is_not_zero() {
        let i = OmoyaIntrospect::default();
        let first = i
            .request_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        assert_eq!(first, 1, "a zero id would collide with 'never requested'");
    }

    /// A region request carries its rectangle rather than silently capturing
    /// the whole output — the failure mode where a caller asks for 100x100 and
    /// reasons about a full-screen image.
    #[test]
    fn a_region_request_keeps_its_rectangle() {
        let r = CaptureRequest {
            id: 7,
            path: "/tmp/x.ppm".into(),
            region: Some((10, 20, 30, 40)),
            hash_only: true,
        };
        assert_eq!(r.region, Some((10, 20, 30, 40)));
        assert!(r.hash_only);
        // And a full-output request is DISTINCT from a zero-sized region.
        let full = CaptureRequest {
            id: 8,
            path: "/tmp/y.ppm".into(),
            region: None,
            hash_only: false,
        };
        assert!(full.region.is_none(), "None means whole output, not 0x0");
    }
}
