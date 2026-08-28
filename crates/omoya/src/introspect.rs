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
    pub capture_request: std::sync::Mutex<Option<String>>,
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
                if self.backend.load(Ordering::Relaxed) == 1 { "drm" } else { "nested" }
            )),
            // ★ `unknown` is its own arm, not a default to one of the two.
            // A seat that has not modeset yet genuinely does not know, and
            // reporting a guess here would be worse than reporting nothing:
            // the whole point of the leaf is to settle which path is live.
            "atomic" => Ok(serde_json::json!(match self.atomic.load(Ordering::Relaxed) {
                1 => "atomic",
                2 => "legacy",
                _ => "unknown",
            })),
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
                let steps = crate::synth::expand(&synth)
                    .map_err(|e| QueryError::unknown_field(e))?;
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
                        self.queue_input(crate::synth::Synth::Key { code, pressed: true });
                        self.queue_input(crate::synth::Synth::Key { code, pressed: false });
                        Ok(serde_json::json!({ "queued": "tap", "code": code }))
                    }
                    Some(pressed) => {
                        self.queue_input(crate::synth::Synth::Key { code, pressed });
                        Ok(serde_json::json!({ "queued": "hold", "code": code, "pressed": pressed }))
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
                self.queue_input(crate::synth::Synth::Button { code, pressed: true });
                self.queue_input(crate::synth::Synth::Button { code, pressed: false });
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
                    .map_or_else(|| "none".to_string(), |(x, y, w, h)| format!(
                        "{x},{y} {w}x{h}"
                    ))
            )),
            "frame_us" => Ok(n(&self.frame_us)),
            "blit_fast" => Ok(n(&self.blit_fast)),
            "blit_slow" => Ok(n(&self.blit_slow)),
            "blit_general" => Ok(n(&self.blit_general)),
            "gather_us" => Ok(n(&self.gather_us)),
            // ★ WHICH MODE IS LIVE. Reading `td_dirty_pct` without this is a
            // measurement with no idea whether it changed anything: `verify`
            // publishes identical counters while leaving the screen untouched.
            "td_mode" => Ok(serde_json::json!(
                match self.td_mode.load(std::sync::atomic::Ordering::Relaxed) {
                    0 => "off",
                    2 => "verify",
                    _ => "on",
                }
            )),
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
                        let was = self.td_mode.swap(m.to_u64(), std::sync::atomic::Ordering::Relaxed);
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
            // The one number an operator actually wants: what fraction of the
            // surface a commit really changes. Derived here rather than left
            // to the caller so the denominator cannot be dropped on the way.
            "td_dirty_pct" => {
                let ex = self.td_rows_examined.load(std::sync::atomic::Ordering::Relaxed);
                let di = self.td_rows_dirty.load(std::sync::atomic::Ordering::Relaxed);
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
                self.modes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
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
                *self.capture_request.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(path.to_string());
                *self.capture_result.lock().unwrap_or_else(|e| e.into_inner()) = None;
                // Same pairing as `do` above: owe the frame, then wake. A
                // screenshot of an idle seat is precisely the case where no
                // client is going to commit on our behalf.
                self.mark(crate::owed::Owed::Capture);
                if let Some(p) = self.wake.get() {
                    p.ping();
                }
                Ok(serde_json::json!({ "requested": path }))
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
            "stale_result" => Ok(serde_json::json!(
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
            "blit_fast",
            "blit_general",
            "blit_slow",
            "gather_us",
            "window_app_ids",
            "td_mode",
            "td_refined",
            "td_refused",
            "td_rows_dirty",
            "td_rows_examined",
            "td_shadows",
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
            let Some(rest) = t.strip_prefix('"') else { continue };
            let Some((name, after)) = rest.split_once('"') else { continue };
            if !after.trim_start().starts_with("=>") {
                continue;
            }
            const VERBS: &[&str] = &[
                "do", "type", "key", "pointer", "click", "capture",
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
    }
}
