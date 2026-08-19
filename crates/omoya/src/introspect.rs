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
    Arc,
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
    /// Frames the render loop has queued since start.
    pub frames: AtomicU64,
    /// Windows currently mapped in the space.
    pub windows: AtomicU64,
    /// Reserved chords recognised but not acted on — the M4 debt counter.
    pub owed_vt_switches: AtomicU64,
    /// Scanout width/height, 0 when nested or not yet known.
    pub output_w: AtomicU64,
    pub output_h: AtomicU64,
    /// Refresh in Hz, as the panel reported it.
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
            "frames" => Ok(n(&self.frames)),
            "windows" => Ok(n(&self.windows)),
            "owed_vt_switches" => Ok(n(&self.owed_vt_switches)),
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
            "windows",
            "owed_vt_switches",
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
    fn an_unknown_leaf_is_refused_by_name() {
        let s = OmoyaIntrospect::default();
        match s.query(&Query::field(vec!["nonesuch".into()])) {
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

        let v = s.query(&Query::field(vec!["seat".into()])).unwrap();
        assert_eq!(v["backend"], "drm");
        assert_eq!(v["input_attached"], true);
        assert_eq!(v["output"]["width"], 1024);
        assert_eq!(v["windows"], 3);
        assert_eq!(v["frames"], 1);
    }
}
