//! MCP face for omoya — the compositor, driveable by an agent.
//!
//! ── ★ WHY THIS EXISTS ───────────────────────────────────────────────────
//! omoya already served its whole surface over kanshou: 42 read leaves and
//! 7 write verbs (synthetic keyboard, pointer, click, screenshot) against
//! the LIVE seat. None of it was reachable by an agent, because reaching it
//! meant hand-framing u32-BE-length-prefixed JSON at
//! `$XDG_RUNTIME_DIR/kanshou/omoya-<pid>.sock`. So the single most valuable
//! thing on the desktop — the desktop itself — was the one component with
//! no MCP face, while mado, tear, tend and frost all had one.
//!
//! ── ★ WHY ~10 TOOLS AND NOT 49 ──────────────────────────────────────────
//! mado spends 5,817 lines on 63 tools, most of it mechanical: one
//! hand-written `#[tool]` per leaf. Repeating that here would cost ~2,000
//! lines to say "forward this path" forty-two times, and every future leaf
//! would owe a tool.
//!
//! The read surface is therefore ONE typed tool over a leaf NAME
//! (`omoya_read`), plus `omoya_leaves` to enumerate what may be passed to
//! it. That is not a shortcut — it is the shape kanshou's own protocol
//! already has (a `Query` IS a path plus args), so the tool surface mirrors
//! the wire surface instead of flattening it into N copies.
//!
//! The WRITE verbs get individual tools, because they are the ones with
//! real arguments, real failure modes, and real consequences on an
//! operator's live seat — those deserve typed parameters and their own
//! descriptions, not a stringly-typed escape hatch.
//!
//! ── ★ THE CATALOG IS A CONSTANT HERE, AND THAT IS A KNOWN DEBT ──────────
//! `LEAVES` below is hand-maintained because kanshou's `schema()` is never
//! dispatched over the wire — `handle_connection` routes `query()` only, so
//! a `schema` query answers `unknown-field`. Measured 2026-08-27 against
//! plo. When that gap closes, `omoya_leaves` should ask the live compositor
//! instead of reading this constant, and this comment is the marker for
//! that change. Until then the constant is the honest option: a wrong
//! catalog is visible (the leaf simply refuses), whereas a derived catalog
//! that silently returns nothing is not.
//!
//! ── ★ EVERY ANSWER IS A kotae OUTCOME ───────────────────────────────────
//! `found` / `empty` / `refused` / `blind` never render the same bytes. The
//! distinction that matters most here is **blind**: no live compositor is
//! not the same as a compositor that answered "no". An agent asked to
//! troubleshoot a seat must never read "no windows" when the truth is "no
//! omoya is running".

use kanshou::Query;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;

/// The app name kanshou discovers by — matches the socket basename
/// `omoya-<pid>.sock` that `introspect::serve` binds.
const APP: &str = "omoya";

/// Every read leaf `introspect.rs` answers. See the module header on why
/// this is a constant rather than a live `schema()` query.
const LEAVES: &[&str] = &[
    "backend", "blit_fast", "blit_general", "blit_slow", "capture_result",
    "chord_deeds", "deeds_performed", "elements", "focus_rect", "frame_us",
    "frames", "gather_us", "geometry", "import_full", "import_partial",
    "input_attached", "input_devices", "last_frame_causes", "layout", "mode",
    "modes", "output", "owed", "owed_causes", "owed_vt_switches", "pointer",
    "presented", "seat", "session_active", "session_events", "socket",
    "synth_performed", "td_dirty_pct", "td_mode", "td_refined", "td_refused",
    "td_rows_dirty", "td_rows_examined", "td_shadows", "verbs",
    "window_app_ids", "windows",
];

/// Ship a query to the live compositor and render the kotae outcome.
///
/// `blind` is returned when kanshou discovers no live omoya — deliberately
/// NOT an empty success. An agent diagnosing a dark seat must be able to
/// tell "the compositor says zero windows" from "there is no compositor".
async fn ask(path: Vec<String>, args: Vec<serde_json::Value>) -> String {
    let q = Query { path: path.clone(), args };
    let outcome = kanshou::mcp::forward_status(APP, &q, || {
        Err(kanshou::QueryError::unknown_field("no live omoya"))
    })
    .await;

    match outcome {
        kanshou::mcp::ForwardOutcome::Live { pid, value } => serde_json::json!({
            "outcome": "found",
            "omoya_pid": pid,
            "query": path.join("/"),
            "value": value,
        })
        .to_string(),
        // No socket, or every discovered socket was stale. This is the
        // `blind` arm: we did not learn that the answer is nothing, we
        // learned that nobody answered.
        _ => serde_json::json!({
            "outcome": "blind",
            "query": path.join("/"),
            "reason": "no live omoya reachable over kanshou on this host",
            "hint": "omoya must be running as the seat's compositor; check `pgrep omoya` \
                     and $XDG_RUNTIME_DIR/kanshou/omoya-<pid>.sock",
        })
        .to_string(),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadInput {
    /// The leaf to read. Call `omoya_leaves` for the full list.
    pub leaf: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VerbInput {
    /// The verb to perform, e.g. `focus-right`, `close`, `spawn-terminal`.
    /// Call `omoya_verbs` for the closed vocabulary.
    pub verb: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TypeInput {
    /// Text to type into the focused surface. `\n` submits.
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KeyInput {
    /// evdev keycode, e.g. 28 = KEY_ENTER, 1 = KEY_ESC.
    pub code: u32,
    /// Omit for a tap (press + release, which cannot strand a modifier).
    /// Set true/false only when deliberately holding or releasing.
    pub pressed: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PointerInput {
    /// Relative X motion in logical pixels.
    pub dx: f64,
    /// Relative Y motion in logical pixels.
    pub dy: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClickInput {
    /// evdev button code. Defaults to 272 (BTN_LEFT). Press and release are
    /// both queued, so a caller cannot leave a button down on the seat.
    pub code: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CaptureInput {
    /// Absolute path ON THE HOST RUNNING omoya to write the screenshot to.
    pub path: String,
}

#[derive(Clone)]
pub struct OmoyaMcp {
    tool_router: ToolRouter<Self>,
}

impl Default for OmoyaMcp {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl OmoyaMcp {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    // ── reads ───────────────────────────────────────────────────────────

    #[tool(
        description = "The whole seat in one call: backend, mode, elements, frames, geometry, \
                       layout. START HERE when diagnosing a desktop — it answers 'is there a \
                       compositor, what is it driving, and is it painting' in a single query."
    )]
    async fn omoya_seat(&self) -> String {
        ask(vec!["seat".into()], vec![]).await
    }

    #[tool(
        description = "Read one introspection leaf from the live compositor. Every read leaf \
                       omoya serves is reachable through this one tool; call omoya_leaves for \
                       the list. Returns kotae outcomes: found (with the value), or blind when \
                       no compositor is running — never a zero that could be mistaken for one."
    )]
    async fn omoya_read(&self, Parameters(input): Parameters<ReadInput>) -> String {
        ask(vec![input.leaf], vec![]).await
    }

    #[tool(
        description = "List every readable leaf. Note this is omoya's own catalog constant, \
                       not a live schema query — kanshou does not dispatch schema() over the \
                       wire, so a live catalog is not available yet."
    )]
    async fn omoya_leaves(&self) -> String {
        serde_json::json!({
            "outcome": "found",
            "count": LEAVES.len(),
            "leaves": LEAVES,
            "caveat": "static catalog; kanshou schema() is not wire-dispatched",
        })
        .to_string()
    }

    #[tool(description = "List the compositor's closed verb vocabulary, from the LIVE seat.")]
    async fn omoya_verbs(&self) -> String {
        ask(vec!["verbs".into()], vec![]).await
    }

    // ── writes: these act on a real operator's live desktop ──────────────

    #[tool(
        description = "Perform a compositor verb on the live seat (focus-left/right/up/down, \
                       resize-*, close, spawn-terminal, spawn-launcher). An unknown verb is \
                       REFUSED by name rather than ignored. MUTATES the operator's desktop."
    )]
    async fn omoya_do(&self, Parameters(input): Parameters<VerbInput>) -> String {
        ask(vec!["do".into(), input.verb], vec![]).await
    }

    #[tool(
        description = "Type text into the focused surface via synthetic input. Validated before \
                       queueing, so an unmappable character is refused to the caller instead of \
                       being dropped later. The answer says 'queued' — read synth_performed to \
                       confirm it landed. MUTATES the operator's desktop."
    )]
    async fn omoya_type(&self, Parameters(input): Parameters<TypeInput>) -> String {
        ask(vec!["type".into()], vec![serde_json::json!(input.text)]).await
    }

    #[tool(
        description = "Send an evdev keycode. Omit `pressed` for a tap — that cannot strand a \
                       held modifier on the seat. MUTATES the operator's desktop."
    )]
    async fn omoya_key(&self, Parameters(input): Parameters<KeyInput>) -> String {
        let mut args = vec![serde_json::json!(input.code)];
        if let Some(p) = input.pressed {
            args.push(serde_json::json!(p));
        }
        ask(vec!["key".into()], args).await
    }

    #[tool(description = "Move the pointer by a relative delta. MUTATES the operator's desktop.")]
    async fn omoya_pointer(&self, Parameters(input): Parameters<PointerInput>) -> String {
        ask(
            vec!["pointer".into()],
            vec![serde_json::json!(input.dx), serde_json::json!(input.dy)],
        )
        .await
    }

    #[tool(
        description = "Click a pointer button (default BTN_LEFT/272). Press and release are both \
                       queued. MUTATES the operator's desktop."
    )]
    async fn omoya_click(&self, Parameters(input): Parameters<ClickInput>) -> String {
        let args = input
            .code
            .map(|c| vec![serde_json::json!(c)])
            .unwrap_or_default();
        ask(vec!["click".into()], args).await
    }

    #[tool(
        description = "Screenshot the live seat to a path ON THE COMPOSITOR'S HOST. The single \
                       most useful tool for diagnosing a desktop remotely: it answers what the \
                       screen actually shows, which no counter can."
    )]
    async fn omoya_capture(&self, Parameters(input): Parameters<CaptureInput>) -> String {
        ask(vec!["capture".into()], vec![serde_json::json!(input.path)]).await
    }
}

#[tool_handler]
impl ServerHandler for OmoyaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "omoya (母屋) — the pleme-io Wayland compositor. Read the seat's live state and \
                 drive it with synthetic input. Answers carry kotae outcomes: `blind` means no \
                 compositor is running, which is never the same as an empty result."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Run the MCP server over stdio.
///
/// stdout is the JSON-RPC framing channel, so every diagnostic must go to
/// stderr — a single stray println here corrupts the protocol for the whole
/// session.
pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let service = OmoyaMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_leaf_is_unique_and_sorted_enough_to_read() {
        let mut seen = std::collections::HashSet::new();
        for l in LEAVES {
            assert!(seen.insert(*l), "duplicate leaf in catalog: {l}");
        }
        assert!(LEAVES.len() >= 40, "catalog shrank unexpectedly: {}", LEAVES.len());
    }

    /// The write verbs must NOT appear in the read catalog — `omoya_read`
    /// is documented as a read surface, and a caller who found `type` in
    /// the leaf list would reasonably expect reading it to be harmless.
    /// ★ `pointer` IS DELIBERATELY ABSENT FROM THIS LIST, AND THAT IS THE
    /// POINT OF THE COMMENT.
    ///
    /// The first version of this test forbade every write verb from the read
    /// catalog, `pointer` included, and it FAILED — correctly. `pointer` is
    /// overloaded in omoya's own protocol: read with no args it reports where
    /// the pointer is, and called with `dx`/`dy` it moves it. One name, two
    /// operations, distinguished by arity.
    ///
    /// So the invariant is not "no write verb is readable" — that was my
    /// assumption and the seat disagreed. It is "no write verb is listed as a
    /// readable leaf UNLESS it genuinely answers a read", which for this
    /// surface is `pointer` alone. Encoding the wrong rule here would have
    /// forced someone to delete a real leaf to make a test pass.
    #[test]
    fn read_catalog_excludes_the_write_only_verbs() {
        for verb in ["do", "type", "key", "click", "capture", "td_mode_set"] {
            assert!(
                !LEAVES.contains(&verb),
                "write-only verb `{verb}` must not be listed as a readable leaf"
            );
        }
        assert!(
            LEAVES.contains(&"pointer"),
            "`pointer` answers a read (the position) and must stay in the catalog"
        );
    }
}
