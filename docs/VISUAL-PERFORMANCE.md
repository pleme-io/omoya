# Visual performance on the pleme-io seat — the master plan

> **Status: PLAN, with Round 0 measurements taken live.** Every number below
> was read off plo on 2026-08-21 through omoya's kanshou socket, not estimated.
> Rounds are ordered by measured payoff, and each one's done-predicate is a
> number, never a claim.

---

## I. The philosophy — **hitofude** (一筆), "one stroke"

*Name PROPOSED, not ratified. The `naming` skill's corpus check is documented
to fail silently, so treat this as a working handle until it has been run.*

**A pixel is written once, by whoever owns it, and never copied again.**

That is the whole doctrine, and it is not an aesthetic preference — it is what
the measurements force. The seat's slow path is not slow *code*. Every stage in
it runs at memory-bandwidth speed and 96.3% of its blit rows take the plain
`memcpy` path. It is slow because **the same eight megabytes are walked three
times to put one character on the screen.**

Four laws follow, in the order they bind:

1. **Cost is proportional to what CHANGED, never to what EXISTS.**
   A one-glyph edit that costs a full-surface traversal is the defect, and no
   amount of making the traversal faster fixes it. Find the stage that lost the
   damage rect, and give it back.

2. **Damage is a TYPE, not a hint.** It must be *carried* through every layer,
   never recomputed and never widened by default. A layer that cannot express
   "only this rectangle changed" will silently say "everything did", and the
   two are indistinguishable downstream. (`mekuri`'s `Verdict::{Draw(Pass),
   Skip}` already does this for *whether* to draw; the rounds below extend it
   to *how much*.)

3. **The display drives the schedule; a clock never does.** A periodic tick
   imposes its own interval as a latency floor on everything arriving between
   ticks, and pays CPU for every tick that finds nothing. Measured here:
   **2,782,048 loop ticks against 11,618 presentations** — the gate is
   discarding 99.58% of them correctly, and the ticks are still a poll.
   *Polling is an autoloss.*

4. **Every claim carries its denominator.** "Fast path" means nothing without
   the slow-path count beside it. `blit_fast` / `blit_slow` / `blit_general`
   already ship as a triple for exactly this reason, and every probe added
   below must too — a counter that can only go up cannot report a regression.

**The anti-goal, stated so it can be argued with:** making a copy faster is
never the win. Deleting it is. Any round that ends with "the memcpy is now
SIMD" and the same number of passes has failed on its own terms.

---

## II. Round 0 — the measurements (TAKEN, 2026-08-21, plo, omoya pid 3823008)

Read live through kanshou. This section is the baseline every later round is
scored against; re-measure rather than citing it.

### II.1 Frame cost splits 68× by cause

| cause | n | min | **median** | max |
|---|---|---|---|---|
| pointer-only motion | 12 | 52 µs | **60 µs** | 76 µs |
| one-character text commit | 12 | 4,047 µs | **4,099 µs** | 4,384 µs |

The display is **1920x1080@360** (selected from
`1920x1080@60 360* 300 240 144 120`), so one interval is **2,778 µs**.

**A text-commit frame costs 1.47 display intervals.** Every keystroke misses
its vblank by construction — not occasionally, always. That single ratio is
what this whole plan exists to move, and it is why Round 2 (scheduling) must
come *after* Round 1: making the scheduler tighter while a frame cannot fit in
an interval measures *worse*, not better.

### II.2 The traffic, counted

The content surface is **1912×1044** (from `geometry`), i.e. 1,996,128 px ×
4 B = **7.98 MB**. Reading the code path for one text commit:

| # | stage | what it walks |
|---|---|---|
| 1 | `import_partial` → `buf[a..b].copy_from_slice(&bytes[a..b])` | the damaged rows, client SHM → nuri texture |
| 2 | `normalise_opaque(&mut buf[a..b], fourcc)` | the same rows again, read + write |
| 3 | `render_output` → `nuri::blit` | texture → mapped dumb buffer |

Three full passes ≈ **24 MB per keystroke**. At the ~6 GB/s a single core
sustains against cold cache, that is ~4 ms — which is the 4,099 µs measured,
to within the noise. **The arithmetic closes.** Nothing here is mysterious and
nothing here needs a profiler to find.

### II.3 ★ The root cause is upstream of all three: **mado does not send buffer damage**

`mado/src/grid_damage.rs:33` says it outright — *"`wl_surface.damage_buffer` —
presentation goes through wgpu"*. wgpu's `present()` takes no damage argument,
so winit's Wayland backend commits **full-surface damage every frame**.

So mado computes a perfectly good damage rect, uses it for its own draw, and
then tells the compositor the whole window changed. Every stage in §II.2 is
proportional to that lie. **They are not three problems. They are one problem
counted three times**, and it is a client-side problem being paid for by the
compositor.

This is the correction that matters most in this document: an earlier reading
of the same gap blamed the compositor's pass count. The passes are fine. The
damage is wrong.

### II.4 The counters that are already healthy — do not "optimize" these

| counter | value | reading |
|---|---|---|
| `gather_us` | 20 µs | element gather is free; it was 693,952 µs before the damage gate |
| `blit_fast` / `blit_slow` / `blit_general` | 10,083,509 / 383,320 / **0** | 96.3% plain-memcpy rows, zero general path |
| `import_full` / `import_partial` | 15 / 9,301 | partial import dominates 620:1 |
| `owed` | false | the ledger settles; no frame is stuck owed |
| `elements` | 6 | cursor + bar + 4 borders + content |

A round that touches any of these without a number showing they hurt is
optimizing where the cost is not.

---

## III. The layers — divided so each is testable ALONE and IN COMPOSITION

The operator's requirement, and it is also the only reason §II.1 was findable:
a whole-stack number cannot tell you *which* stage lost the damage rect.

Each layer gets **(a)** a bench harness over a fixed corpus with a pinned
budget, **(b)** a typed probe set published over kanshou with its denominator,
and **(c)** a row in the whole-stack matrix. A layer with no isolated harness
is a layer whose regressions are attributed to its neighbour.

| L | layer | owns | isolated test | composed test |
|---|---|---|---|---|
| **L0** | **Scanout** — DRM/KMS, mode, planes, page-flip | which mode, which planes, when the flip lands | vkms VM test (exists, `checks.vkms-seat`) | input→photon latency |
| **L1** | **Raster** — `nuri` | blit, blend, format normalisation | per-kernel bench over a rect corpus (**absent**) | frame_us by cause |
| **L2** | **Composite** — omoya's damage tracker + `mekuri` | which elements, which rects, whether to draw at all | mekuri ledger tests (exist) | presented/ticks ratio |
| **L3** | **Schedule** — calloop loop, vblank, frame callbacks | when a frame starts, when clients are told to draw | deadline-model unit test (**absent**) | miss rate vs vblank |
| **L4** | **Client — mado** | grid damage, wgpu present, PTY wake | `grid_damage` tests (exist) | frame_us on text commit |
| **L5** | **Client — Chrome** | its own GPU path, VizDisplayCompositor | none — third-party | frame_us with Chrome mapped |
| **L6** | **Decor** — bar, borders, focus ring, cursor | 1920×28 bar, 4×2px borders, 20×34 cursor | bar render golden (exists) | elements count, chrome-cause frames |
| **L7** | **Colour + text** — Nord roles, sRGB↔linear, glyph blend, font discovery | gamma-correct coverage blending | golden-frame hash (exists) | pixel-exact capture diff |
| **L8** | **Input** — evdev, chord, deed | keycode→deed, repeat gate | chord seam tests (exist, extended 2026-08-21) | input→photon latency |

**The whole-stack number is one: input event → photon.** Everything else is a
diagnostic for it. It does not exist yet and Round 0b builds it.

---

## IV. The rounds

Ordered by measured payoff. Each states its done-predicate as a number.

### Round 0b — the end-to-end probe *(prerequisite for scoring anything)*
Build `input→photon`: stamp the evdev event, carry the stamp through the deed
and the composite, and close it on the page-flip completion event. Publish as a
`hot::Hist` (already in `pleme-observability`, `BUCKETS=128`, `SUB_BITS=2`)
over kanshou, with `sampled=n/offered` so the sampling denominator is inside
the value.
**Done:** a p50/p99 for keystroke→photon exists and is queryable remotely.

### Round 1 — **make the damage true** *(the 68×)*
The whole of §II.3. mado sends real `wl_surface.damage_buffer` rects derived
from the `grid_damage` it already computes. wgpu will not do it, so the
surface must be damaged beside the present — which means reaching the
`wl_surface` under winit, or presenting through a path that admits damage.
All three compositor passes then shrink to the damaged rect *for free*,
because all three are already written against `damage`.
**Done:** a one-character text commit costs **< 500 µs** median (from 4,099),
i.e. comfortably inside one 2,778 µs interval, with `blit_fast` still ≥ 95%.

### Round 2 — **event-driven scheduling** *(the autoloss)*
Retire the calloop frame Timer for `DrmDeviceNotifier` vblank events, and
compute a *deadline* — start the frame as late as safely possible before
vblank (Mutter's dynamic-max-render-time is the reference shape), using the
Round 0b histogram as the predictor. A one-shot deadline armed by an event is
not a poll; a heartbeat is.
**Done:** ticks-per-presentation falls from **239:1** to **< 2:1**, and
keystroke→photon p99 falls by at least one display interval.

### Round 3 — **stop copying at all: planes and direct scanout**
- **Hardware cursor plane.** The cursor is currently a composited element
  (`geometry` lists it). A cursor on its own plane costs the compositor
  nothing and removes the most frequent damage source on an idle desktop.
- **Overlay plane for the bar.** 1920×28 that changes once a minute has no
  business being re-composited with the content.
- **Direct scanout of the client buffer.** Blocked today: smithay maps
  `UnderlyingStorage::Memory ⇒ None`, so a CPU-mapped client buffer can never
  be handed to the scanout engine. The honest statement is that this needs
  dmabuf-backed clients, and that is a *client* change, not a compositor one.
**Done:** pointer-only frames stop reaching `nuri` at all (`presented` no
longer increments on bare motion); bar-only frames do not touch the content
plane.

### Round 4 — **retire the `target_fps` stopgap**
mado is pinned to `target_fps = 180` on plo because it has no wake signal. The
real fix is `madori`'s PTY wake — an `EventLoopProxy` fired on terminal
sequence-number change — so the terminal draws when the terminal changes and
sleeps otherwise.
**Done:** mado's idle CPU with a static screen is < 1% of a core with
`target_fps` unset, and a keystroke still lands within one interval.

### Round 5 — **the colour and text pipeline**
- SIMD the blend kernel (383,320 slow rows and climbing) — but only after
  Round 1, when that count is against a *small* rect and therefore honest.
- Remove the fontconfig XML scan: omoya substring-scans `/etc/fonts` for
  `<dir>` without linking fontconfig, so `<include>`, `<selectfont>` and
  conditionals are silently missed. Nix knows the font paths at build time;
  feed them in.
**Done:** `blit_slow` cost per row halves on a fixed corpus; font discovery
has zero `/etc/fonts` reads.

### Round 6 — **multi-client, with Chrome on the seat**
Chrome landed on plo in the same wave as this plan. It is a large, opaque,
GPU-backed client and it will dominate L5. Measure before designing.
**Done:** frame_us with Chrome mapped and animating stays within one interval.

---

## V. Rust and tatara-lisp — the specific exploitations

Not "use Rust well" — the concrete moves this stack makes available.

### V.1 Rust

| move | what it buys, here |
|---|---|
| **Coordinate-space newtypes** (`#[repr(transparent)]` over `BufferPx`, `OutputPx`, `LogicalPx`) | the mis-blit class. Today a rect in the wrong space compiles and renders subtly wrong; typed, it does not compile. This is the highest-value type change in the stack and it costs zero runtime. |
| **`Refined<T, Bounds>`** for damage rects | an unclipped rect becomes unrepresentable at the parse boundary rather than clamped downstream. The input-resilience triad already uses this shape for font size and key repeat. |
| **Const generics over pixel format** | monomorphise `blit` per `Fourcc` so the per-pixel format branch vanishes. `blit_general` is already 0, so this is about keeping it there as formats are added, not about today's cost. |
| **`std::simd` on the blend kernel** | Round 5 only. 383K slow rows is real but it is 3.7% of rows; do it when it is 3.7% of a *small* number. |
| **Lifetime-tied mappings** | `NuriFramebuffer<'a>` already ties the slice to the `DmabufMapping`. Extend to make a *persistent* mapping safe — today the per-frame map/unmap acts as an accidental store-drain barrier, and removing it without the lifetime story is a tearing bug. |
| **`#[must_use]` on every damage-returning fn** | the `add_binding` lesson from awase, applied to rects: a discarded damage rect is a stale region on screen, and it is silent. |

### V.2 tatara-lisp

The fluidity to exploit is that **a render path is DATA**, and choosing one is
a decision over a small typed space — exactly what a `(def…)` form is for.

- **`(defrenderpath …)`** — declare each stage and the `(buffer-kind, damage-shape,
  format, plane-availability)` tuple it serves. The interpreter picks the
  shortest legal path for a frame instead of the code branching its way there.
  A path that cannot serve a tuple has no arm, so "we fell through to the slow
  path" becomes a typed refusal rather than a silent detour.
- **`(defbench …)`** — the ★★ CLOSED-LOOP MASS-SYNTHESIS matrix: every stage ×
  every variant, **failing the build when a variant lands without a row**. This
  is what makes §III's "tested apart and as a whole" enforceable rather than
  aspirational.
- **`(defprobe …)`** — the typed probe set, so a counter and its denominator are
  declared together and a denominator-less counter has no constructor.

**Two measured traps, from `theory/TATARA-LISP-SURFACE.md`, that this design
must respect:** a typo'd keyword yields an **empty `Vec` reported as success**
(so every form must assert a compiled count and refuse zero), and there is
**no computation at the compile layer** — no arithmetic, no lambda. So derived
values (a deadline from a histogram, a stride from a width) are computed in
Rust or in Nix, never in the Lisp.

---

## VI. What this plan deliberately does NOT do

- **It does not adopt engawa/garasu.** engawa is a pure-data render-graph IR
  with no rasterizer and its only `Dispatcher` impl is a test tape; garasu is
  wgpu-only. Neither has a textured-quad primitive, a scissor/damage type, or a
  multi-writer story — and N clients onto one output *is* N writers. Named as a
  deliberate non-goal so it is not rediscovered as an oversight.
- **It does not chase `blit` throughput before Round 1.** See §I's anti-goal.
- **It does not touch the counters in §II.4.**

---

## VII. Honest ledger

| claim | tier |
|---|---|
| the 68× split, the mode, the traffic arithmetic | **measured** on plo 2026-08-21 |
| mado not sending buffer damage | **measured** — its own source says so |
| direct scanout blocked by `UnderlyingStorage::Memory ⇒ None` | **measured** in smithay's source |
| every round's done-predicate | **unmet** — this is a plan |
| `hitofude` as a name | **proposed, unratified** — run the `naming` corpus check |
| the layer table's "isolated test" column | **two are absent** (L1 raster bench, L3 deadline model) and say so |
