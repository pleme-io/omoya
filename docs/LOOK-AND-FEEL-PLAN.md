# The desktop look-and-feel plan — "Nord macOS"

> **Generated 2026-09-03** from a 13-agent recon + research + adversarial-review
> pass (2.77M tokens, 424 tool calls). Every path-bearing claim below was read
> from source or fetched from an upstream doc. Re-measure before acting on any
> number — this is a dated snapshot, and it already contains one correction to
> its own recon.
>
> **Read [`SHITSURAI.md`](./SHITSURAI.md) first.** This document does not
> replace it. It revises exactly two of its premises — both of which went stale
> when plo flipped to floating mode — and otherwise extends it.

---

## 0. The finding that reframes everything

The operator's report was *"the look and feel is absolutely just bad"*, and the
instinct — mine, and the instinct of every ricing guide — was **gaps, rounding,
shadows, blur**. That instinct is wrong here, and the measurement says so
unambiguously:

> **The desktop ground, every mado window, and tobira all paint the
> byte-identical `#2E3440`.** A contrast ratio of **1.00:1**. Nothing on screen
> has an edge.

Three independent adversarial reviewers reached this conclusion separately.
It explains every symptom at once:

- a floating window with no visible boundary (recorded in `layout.rs` already:
  *"a mado window's background is nord0 and the desktop ground is nord0, a
  floating window had NO visual boundary whatsoever"*)
- a status bar that vanishes — `nord1` on `nord0` is **1.24:1** across
  1920×28 px, the largest single thing omoya draws
- a launcher that reads as blank on an empty query (it is not blank; it is a
  nord0 panel on a nord0 ground with a caret)

**No amount of space between two surfaces of the same colour creates an edge.**
That is why raising `GAP` from 4 → 12 did nothing but make the screen emptier,
and why it was reverted.

★ **The reference desktops agree.** Of the three best-in-class riced Wayland
compositors, **two (SwayFX, niri) ship with every effect OFF** and read well on
a gap plus a coloured focus ring alone. Effects are not what makes them look
good. Luminance separation is.

---

## 1. What is refuted — written down so it is not re-proposed

Every item on the ricing wishlist was measured against `nuri`'s benchmarked
**2.023 ns/px** and the seat's **2.78 ms** frame budget. This section exists so
the next person (or agent) does not spend a day re-deriving it.

| effect | verdict | the number that settles it |
|---|---|---|
| **Background blur** | **unreachable** | Not "needs a shader we lack" — needs a **GPU that was deliberately removed**. `backend_gbm` removed, `renderer_pixman` replaced by `nuri` (zero-dependency CPU rasterizer), dmabuf withheld. Dual-Kawase is **453% of one frame budget** for a single window. |
| **Drop shadows** | **refuted on outcome, not cost** | The strongest form of refutation. niri's shipped default (softness 30, spread 5, offset y5, `#0007`) composited over `#2E3440` yields `#20252E` — **1.23:1**. That is *less separation than the free `nord0→nord1` step*, which costs nothing. A shadow here buys less than a colour choice. |
| **Window transparency / dim-inactive** | **unreachable** | `nuri::Surface::blit`'s fast path requires `alpha >= 1.0`; below it the blit drops off `copy_from_slice` into a per-pixel loop. One full-screen alpha pass = **2.004 ms = 72% of the frame budget**. |
| **Animations** (open/close/move/resize/workspace slide) | **rejected** | At 360 Hz a 200 ms animation is **72 forced composites**. Worse, it attacks `mekuri`'s value-change damage gate — the primitive this repo just built, which is what makes an idle seat cost zero frames. |
| **Rounded corners, tiled windows** | **rejected** | libadwaita's own rule: radius → 0 when tiled or maximized, because rounding a tiled window cuts holes that read as a defect in the tiling. |
| **Wallpaper** | **rejected, with numbers** | Flat fill **0.070 ms** (vectorized to memset) vs full-screen blit **0.626 ms** — **9× for the same pixels**. |

### 1.1 The two premises that DID go stale

`SHITSURAI` was written when **omoya was a tiler**. plo runs
`layout.mode = "floating"` today. Two of its rejections are conditioned on
tiling and no longer bind:

1. **Rounded corners.** §5.5 rejects radius *"because omoya is a tiler"* and in
   the same breath **reserves `Radius::md` = 8 for "genuinely floating surfaces
   (future launcher, notification)"**. The launcher is now real and floating.
   By shitsurai's own logic this is sanctioned, not forbidden.
2. **Window chrome.** The titlebar only draws in floating mode
   (`drm.rs:1170`), and the default mode is still `Tiling` (`config.rs:180`).
   A default seat has **no chrome at all** — worth knowing before anyone
   concludes chrome is broken.

---

## 2. The destination

> **Nord macOS = macOS's *structure and interaction model*, painted in Nord,
> derived from one authored source.**

Not macOS's effects — those are refuted above. macOS's *legibility*: every
surface sits at a knowable elevation, every window says what it is, every
control is where you expect it, and the whole thing comes from one place.

**The plan is ADOPTION, not invention.** The recon's central finding:

> pleme-io already has a complete fleet design system — `ishou-tokens` ships
> `Radius`, `Shadow`, `Spacing`, `Typography`, `Motion`, a 19-field
> `FleetDefaults`, and `FleetThemedConfig::from_fleet` with **37 `expect_*`
> drift-guard methods** — and **omoya consumes none of it** for any visual
> value. Every constant in the compositor is a local `pub const`.

So the compliant version of "make the desktop beautiful" is: **adopt the spine
that exists, and delete the hand tables.** Per PENTE §X, *"a phase's
done-predicate is a DELETION"* — a phase that lands a spec, an emitter and a
green test while the hand table survives has **failed**.

---

## 3. What is forbidden — the doctrine boundary

The adversarial doctrine review found that **almost every naive move violates
an existing rule on contact**. Recorded here so the plan cannot drift into them:

| tempting move | rule it breaks |
|---|---|
| Add `services.omoya.settings.theme.background` so the seat is re-themable | `config.rs:29-38` refuses this **in writing**, on Pente grounds — and is correct. One authored source, not a per-machine override. |
| Add a Nord (or darker-than-nord0) hex in the nix repo | The NixOS renderer states the rule for this exact case: *"Adding a new consumer of the palette (a compositor, a greeter, a lock screen) means adding a projection HERE, never a second module that names a colour."* |
| Hardcode radius/shadow/gap as new `pub const` next to `GAP`/`BORDER` | **shatei (射程)**: a fact represented at narrower scope than it must hold **drifts silently** — every site still reads correct. |
| Copy Waybar's 30 / Hyprland's rounding 10 / fuzzel's 15 lines as-is | Operating Principle #3, idiom-first: *"External concepts are acquired via translation through pleme-io primitives. Direct foreign-idiom use is a leak."* |
| Hand-draw bar module #4, #5, #6 | ★★ CLOSED-LOOP MASS-SYNTHESIS: a multi-variant surface needs a **matrix that fails the build when a variant lands without a row**. |
| Invent an `omoya::theme` role table / a desktop-theming crate | PENTE explicitly claims that slot. RENDERING adds: two owners of one question is the defect. |

★ **And a live drift the recon caught:** `bar.rs:69-97` hand-rolls six private
role functions that re-derive `ishou_tokens::SemanticRoles::pleme_dark()`
band-for-band. Five of six match exactly; **`warning` diverges** (omoya
`aurora[2]` vs ishou `aurora[1]`). That is precisely the silent one-band drift
`bar.rs`'s own comment says already cost this repo months on the accent.

---

## 4. The plan

Ordered by **impact per unit of work**, which the leverage review computed
independently of my instinct.

### P0 — Give the ground its own luminance rung  ★ THE single change

**One value.** The desktop ground stops being the same colour clients paint.

- Unit of work: one constant, authored as a **binding**, not a bare const.
- Unit of impact: every pixel not covered by a window, **plus a visible edge
  appearing simultaneously on every window on screen** — focused and
  unfocused — at **zero per-window cost**.

The honest complication, from the Nord research: **Nord has no rung below
`nord0`, and defines no compositor/desktop-root role at all.** Its entire model
is *one application*: a background plus elevated elements. The question "when
two independent processes both paint a base plane, who yields?" is **ours to
decide — Nord cannot be cited either way.**

Two options, both legitimate:
- **(a) Sink the ground.** Mint a below-`nord0` value as an acknowledged
  palette extension. Community precedent exists: `#242933`.
- **(b) Raise the chrome.** Leave the ground at `nord0` and lift every
  omoya-painted surface a rung. Palette-faithful; smaller separation.

**Recommendation: (a).** It is the only option that gives an edge to windows
omoya does not paint — which is all of them. It must land as a `(defface)`
binding in the palette's one home, never as a hex in omoya or in nix.

> `pending-pente: <row>` — this advances a PENTE ledger row or leaves a typed note.

### P1 — Raise the bar off its ground

`nord1` on `nord0` is **1.24:1** across the largest element omoya draws.
Bar ground → the next rung; hairline `nord3` → **1.36:1**. The fix is *which
role constant*, not new code — the rasterizer, the sRGB-correct LUT blend and
the role functions all exist.

Land this **as the `SemanticRoles` adoption** that deletes `bar.rs:69-97`'s six
hand-rolled functions, closing the `warning` divergence in the same commit.
**Done-predicate: those six functions are gone.**

### P2 — tobira: the icon it already computes, and a non-empty empty state

The cheapest operability win in the whole stack.

- **Icons.** The `Icon` type is built and populated by **all 16 providers**
  (`providers/mod.rs:94-104`) and **thrown away at `app.rs:294-297`**. One
  wire. Without it, a 10-row mixed result set has no preattentive way to tell
  an app from a file from a clipboard entry.
  ★ Decide the **sizing school first** — icon == 1em (rofi/fuzzel, dense
  one-line rows) vs icon == fixed 32px (wofi/walker, taller rows) — because it
  sets row height → visible-result count → window height. *"Choosing the icon
  size after the window height is how launchers end up showing 6 results in a
  30% box."*
- **Empty query.** **4 of 5** reference launchers populate the list before a
  keystroke; we show nothing. Two shipped models: fuzzel's `hide-before-typing`
  boolean (minimal) or walker's `[providers] empty = [...]` (a *different*
  source set for the empty state). **The walker model compounds** — it makes
  "what does an empty box mean" a typed field rather than an if-branch.
- **MRU ranking.** An empty list is only useful if ordered by use. All four
  that populate it rank by a usage cache. Alphabetical would be a regression
  against all four.
- **Expunge binding.** fuzzel and rofi **independently converged** on
  `Shift+Delete` to remove a mis-ranked MRU entry. Per the convergent-evidence
  rule: adopt as-is rather than re-derive.

### P3 — Window titles in the titlebar

Every window's 24px bar is currently byte-identical, because **omoya never
reads `xdg_toplevel`'s title** — zero code matches for `.title` across the
crate. N identical bars carry no information. This is `reachable-now`: the bar
rasterizer already exists (`bar.rs`), the chrome rect already exists.

### P4 — Rounded corners on floating surfaces only

The one place the rounding instinct survives contact. §5.5 **reserves**
`Radius::md` = 8 for floating surfaces, and plo runs floating. Cost is an
alpha-blended corner mask — affordable **because omoya composites into a shadow
buffer in ordinary RAM** (`nuri_renderer.rs:252-281`, the Weston model) rather
than into the write-combining scanout mapping where a read costs **~1000×**.

Scope: the launcher and future notifications. **Not tiled windows** — that
rejection still stands.

### P5 — Bar modules (battery, network, volume)

**Highest absolute operability value, lowest impact per unit of work** — and
correctly last. Nothing on this seat tells the operator the battery is at 4% or
that wifi dropped.

★ Must land as a **matrix-gated variant surface**, not as more straight-line
calls in `rasterize_h`. Waybar ships ~30 modules; hand-drawing the 4th is where
CLOSED-LOOP MASS-SYNTHESIS binds.

---

## 5. Tier-honest ledger

<!-- tier-ledger -->

| bad state | how the plan corners it | tier |
|---|---|---|
| desktop and client paint the same colour, so no window has an edge | P0 gives the ground its own rung, authored as a binding in the palette's one home | **not yet built** — the plan's P0 |
| a fleet visual constant drifts between omoya and ishou | adopt `FleetThemedConfig::from_fleet` + a `convergence::Guard`; 37 `expect_*` methods make a `FleetDefaults` change fail omoya's build | only-mitigated (C2 — CI-caught) until adopted; **absent today** |
| `bar.rs`'s six hand-rolled role functions diverge from `SemanticRoles` | P1 deletes them; the divergence is closed by construction | **live defect today** — `warning` already diverges |
| an operator re-themes one machine and the fleet forks | already truly-unrep: `config.rs` refuses a colour knob on Pente grounds | truly-unrep |
| a launcher row is indistinguishable from another provider's | P2 renders the `Icon` every provider already computes | **reachable now** — one wire |
| blur / shadows / transparency proposed again | §1 records the measured refutation with numbers | documentation-only — no gate |

★ **Tier honesty about this document:** nothing in §4 is built. P0–P5 are a
plan, and the ledger says so. The only *shipped* rows are the ones marking
existing refusals.

---

## 6. What this plan corrects about its own recon

Kept because the failure is instructive, and it is the same failure twice.

- The recon agent read `layout.rs:66` as `GAP = 12` and treated the
  `"★ 4, NOT 8"` comment as the stale half. **It was the reverse** — I had
  raised it to 12 hours earlier and reverted it. Two of three reviewers caught
  the staleness independently.
- The agent who raised `GAP` to 12 without reading `SHITSURAI` was **me**, and
  the doctrine review names that class of error precisely: re-deriving a
  decision instead of extending it. The document being ignored was in the same
  repo, argued the exact opposite, and was right.
- I also reported the launcher as *"renders blank"* and built a causal story on
  a GPU fallback. It was rendering correctly — an empty query is a caret on a
  nord0 panel against a nord0 ground. **Typing one word settled it.** A pixel
  histogram I ran to check myself *confirmed the wrong conclusion*, because it
  accurately answered a question I had not asked.

All three are the same shape: **a measurement that is accurate about the wrong
thing.** The 1.00:1 collision in §0 is what all three were circling.
