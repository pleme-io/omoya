<!-- Generated 2026-08-20. Measured, then reviewed by a skeptic and a daily operator.
     Every path-bearing claim was read from source. Re-measure before acting on any
     colour, count, or status claim — this is a dated snapshot.

     ★ ONE CORRECTION ALREADY, made against the machine after the plan was written:
     the GO/NO-GO in section 0 asks whether keystrokes leak to the tty behind the
     seat. They do NOT. /dev/tty1 reports K_OFF, set by logind's TakeControl, which
     omoya calls. omoya itself contains no KDSKBMODE and no EVIOCGRAB, so the plan is
     right that omoya does not do this and wrong about the consequence.

     The REAL consequence is the inverse and is worse: K_OFF means the kernel is not
     processing VT-switch keys either, and change_vt is defined twice and called zero
     times. So Ctrl+Alt+F2 does NOT reach a getty. There is no escape hatch from this
     seat except ssh. That is the finding; keep the second seat for that reason. -->

# NATURALIZE — the full pleme-io desktop

**X = the Linux Wayland desktop environment** — concretely sway/Hyprland + waybar + wofi + mako + swayidle + swaylock + wl-clipboard + grim/slurp + wlr-randr + xdg-desktop-portal. Not "a compositor". The whole seat.

> **GO / NO-GO, before any of this and before plo is anyone's only seat.** `change_vt` is *defined* twice in omoya (`evdev_backend.rs:955`, `logind.rs:434`) and **called zero times** (verified 2026-08-20 — those two definitions are the only occurrences in `omoya/crates/`). `input.rs:65-80` recognises Ctrl+Alt+F<n>, increments `owed_vt_switches`, and forwards. Separately, there is **no `EVIOCGRAB`, no `KDSKBMODE`, no `K_OFF`** anywhere in `omoya/crates/omoya/src` (verified 2026-08-20). Those two facts interact and both branches are load-bearing: with no grab the kernel VT keyboard is still live, so Ctrl+Alt+F2 probably *does* still reach a getty by accident — and **everything typed into this seat is also being delivered to the tty behind it**, which at a future lock screen means the password. Settle it empirically before daily-driving: type a marker string into the seat, switch to VT2, read it back. Until that is settled and written down, plo keeps a second seat.

---

## I. THE DESTINATION — unhedged, before any phasing

**The pleme-io-native desktop is a seat where every pixel, every keystroke, every pointer position and every window placement is a value in a typed pleme-io algebra that an agent can read and drive through the same vocabulary a human's keybinding uses — composited by `omoya` into a write-only scanout it never CPU-reads, laid out by a `kukaku` partition whose parcels are disjoint by construction, painted by `egaku` widgets through a `nuri` drawer with no GPU and no client, dressed by one `irodori`/`ishou` token set, locked by `mukae`'s own credential path with no second implementation of it anywhere, and populated by `noki`, `tobira` and `shirase` as separate layer-shell citizens — with `saihai`'s 284 authored `defaction` rows as the single verb surface behind both the keymap and the MCP write tools, so that driving the desktop by hand and driving it by agent are the same act with two faces.**

Nothing on that sentence is a wrapper. The desktop is not "sway with our theme"; it is the capability re-derived, and the day it is done there is no waybar, no mako, no swaylock, no wofi, no wl-clipboard and no `arboard` in plo's closure.

**Four properties are load-bearing, and every phase below is a path *down from* them:**

1. **The seat never CPU-reads device memory, and never read-modify-writes the scanout.** One invariant, two halves. It is a *type*, not a config value.
2. **Input delivery is not coupled to repaint.** A keystroke reaching a client is a dispatch-completion event, not a frame event.
3. **The seat is drivable without the mouse and recoverable without ssh.** A chord opens a window, a chord closes it, a chord reaches a getty. This is part of the destination, not of the polish.
4. **Every belief the seat holds is published as a typed leaf, and every claim about the seat is a query against one.** Designed in at P0, not bolted on at the end.

**What the destination sentence does NOT cover — stated here so it is a decision, never a discovery.** Cursor rendering and libinput-class device configuration are scheduled (P1/P6). Fractional scaling and HiDPI are scheduled (P10). **Non-US keyboard layouts, compose, dead keys and `zwp_text_input_v3` IME are not scheduled at all** — reported by review at `xkbcommon-hairetsu/src/xkb/mod.rs:219` as a hard `None` for any layout other than `us` and for any variant (cite not independently re-confirmed here — confirm before relying on it either way). If that is permanent it belongs beside XWayland in §IX as a stated consequence; if it is not, it needs a phase. `pending-seat-input: layouts + IME`.

**Path-of-least-resistance check.** The measured latency report offers edits that take the frame from 536 ms to ~85 ms in an afternoon. They land immediately — **and they are not the fix.** Each interim step below names the invariant it stands in for.

---

## II. Citizen vs guest — the bright line, applied

A naturalize that vendors the original has failed. Here is exactly which is which.

### REBUILD (citizen) — vendoring any of these = failed naturalize

| X's thing | pleme-io citizen | status today |
|---|---|---|
| sway/Hyprland/niri layout | **`kukaku`** — **SHIPPED 2026-08-20, and NOT from where this plan expected.** The plan said "`geom` extracted from mado's `float`"; the algebra was actually extracted from **`tear-types`**, which already had the whole split tree — `LayoutNode`/`SplitRatio`/`compute_rects`/`neighbor`/`resize_leaf`, 49 tests, and a `SplitRatio` newtype closing a NaN→zero-extent trap. Its own module header had already named mado as the intended second consumer, so omoya was the third. Better evidence than the plan's guess: kukaku's 49 tests are tear's own cases run against a leaf id kukaku invented, which is what proves the algebra never depended on panes. | **SHIPPED** — `crates/omoya/src/layout.rs`; vkms measures 62500 px of content in each screen-half |
| i3-msg / hyprctl keybinding layer | **`awase`** `BindingMap`/`KeyMode`/`Action`/`Condition`/`Banzuke`/`KeyRepeatGate` | **SHIPPED 2026-08-20** — `deed.rs` is `BindingMap<Deed>`, not a hand-rolled table. Logo+hjkl/arrows focus, Logo+Shift resize, Logo+Q close, Logo+Return terminal. `try_bind` rather than `add_binding`, so a chord bound twice is an error instead of last-wins. A test asserts the seat claims **no** Ctrl or Alt combination — a compositor binding Ctrl+H takes it from every editor at once, and the symptom never points at the compositor |
| pointer cursor | omoya cursor sprite + `wp_cursor_shape_v1` + xcursor theme | ABSENT — `handlers.rs:253` `fn cursor_image(…) {}` is an empty body (verified 2026-08-20). The pointer is invisible |
| VT escape / session control | omoya `change_vt` + an explicit evdev grab decision | **defined, never called** (verified 2026-08-20) |
| waybar / sketchybar | **`noki`** (PROPOSED — see §IV) over ayatsuri's shipped bar model | model SHIPPED (~2.1k objc2-free lines); no Wayland surface exists |
| wofi / fuzzel / rofi | **`tobira`** + a layer-shell surface | SHIPPED (17 providers, shibori fuzzy, okiba XDG); window actions still shell out to `hyprctl dispatch` (`providers/windows.rs:114-125`) — dead on omoya |
| mako / dunst | **`shirase`** + a freedesktop wire + a banner | model/filter/DnD/history SHIPPED; **zero `org.freedesktop.Notifications` anywhere in the fleet**; `render/mod.rs` is `println!` |
| swaylock / gtklock | **`mukae --mode lock`** | `omoya-spec` models `Locked` + `unlock(AuthProof)`; runtime `SeatMode` has only `Entrance`/`Session` (`state.rs:44-49`) and `lock` is *deliberately rejected* as a `--mode` (`:54-56`) |
| swayidle + DPMS + sleep/resume | omoya `idle-notify` + `idle-inhibit` + typed idle policy + `PrepareForSleep` | delegates present in smithay 0.7, unwired; logind device pause/resume IS wired (`logind.rs:309-340`), `PrepareForSleep` is not; no DPMS |
| wl-clipboard / cliphist | **`hasami`** + the three Wayland selection planes | 1 of 3 planes wired; `hikidashi` is a **zero-consumer duplicate** (verified 2026-08-20: only its own `Cargo.toml` mentions it fleet-wide); both wrap `arboard`, which needs a display server |
| grim / slurp / xdg-desktop-portal-wlr | omoya screencopy + `kanshou capture_region` | `capture` writes a PPM to a *path* — useless to an agent on another host |
| wlr-randr / kanshi | omoya output management | ABSENT, and absent from smithay too — the convergent gap |
| GTK/Qt widget toolkits, for our own surfaces | **`egaku`** + a new **`egaku-nuri`** drawer + a shared text stack | egaku SHIPPED (8068 LOC, renderer-free by declared invariant); **one drawer fleet-wide** (`egaku-term`); `nuri` is 485 lines with **zero** `glyph`/`font` symbols (verified 2026-08-20) |
| gsettings / kscreen theming | `irodori` / `ishou` / `pente` | irodori wired into omoya (`theme.rs:15`); ishou has 8 renderers — **no compositor target** |
| i3 workspaces | omoya workspace model + `ext-workspace` | ABSENT, and **40 lines of saihai's catalog mention `workspace`** (verified 2026-08-20; the review counted 41 occurrences — same finding) |
| i3-msg / hyprctl / swaymsg | **`saihai`** `(defbackend omoya)` + MCP tools | **284 `(defaction` rows, zero `(defbackend`** (both verified 2026-08-20) |

### WRAP (guest) — a wire we must speak, correctly

| wire | why it is a wire, not a rebuild | posture |
|---|---|---|
| Wayland protocol codec (smithay 0.7) | the protocol is the interop contract | guest. `omoya/docs/CITIZENSHIP.md` grades omoya a **resident**, not a citizen. **Do not round that up.** |
| DRM/KMS ioctls, evdev, xkbcommon keymap format | kernel + format wires | guest. `hairetsu` emits the keymap |
| logind (D-Bus), greetd socket | session/seat wires | guest — **spoken twice today** (`omoya/src/logind.rs`, `mukae-native/src/logind.rs`, same three constants, identical `zbus 5`). A leaf crate owned by neither is owed |
| `org.freedesktop.Notifications`, xdg-desktop-portal, PipeWire | third-party apps must be able to notify and screen-share us | guest, own the executor. `zbus` is already in omoya's deps |
| **font shaping + glyph rasterization (cosmic-text + swash)** | Unicode shaping and hinted rasterization are a decades-deep external problem, and **the fleet already speaks this wire**: `garasu/src/text.rs` drives `FontSystem`/`Buffer`/`SwashCache` and documents the pipeline as *"cosmic-text resolve → swash raster → glyphon"* (verified 2026-08-20) | **guest, deliberately.** Ours is the *cache, the layout and the blit* — see §IV and P7 |
| **libpam** | **NOT a wire — a C ABI.** `mukae-host` is a guest and must be retired | guest with a **ratchet**: `mukae-native` verifies real shadow hashes (yescrypt + sha-crypt, `!`-refusal) and registers a logind session. Blocked on `/etc/shadow` privilege, not on crypto |
| **XWayland** | the one genuinely-better-wrapped piece; `omoya/docs/SEAT-PLAN.md:389` **excludes it** | **stated consequence, not a silent gap** — see §IX for the corrected population |
| `arboard` (inside hasami/hikidashi) | a guest that must be retired on Linux | replace with native `wl_data_device` + `wlr-data-control` at P3 |

**A third-party client run once as a conformance probe is not vendoring.** `swaybg`, `wofi`, `notify-send`, `wl-paste`, `grim`, `wlr-randr` and **Firefox** appear below only as *differentials*. None of them ships in the seat.

---

## III. Recon — what is genuinely near, stated without the null facts

- **`nuri::fill` and `nuri::blit` both take a `Rect`** (`lib.rs:159`, `:191`). Damage-clipped repaint is expressible in the rasterizer *today*; the blocker is buffer-age bookkeeping in `scanout.rs`.
- **egaku's widget layer is renderer-free by declared invariant** (`lib.rs:33-38`). A compositor can hold those state machines in-process.
- **awase is a dependency omoya already has and uses 1/12 of.** The entire WM keybinding layer is present-but-unwired *in a crate already linked*. This is the single cheapest capability in the plan.
- **The fleet's text stack exists and is CPU-capable.** `swash` rasterizes to **CPU bitmaps** which glyphon merely uploads to a GPU atlas — so a `nuri` drawer reuses the same shaper and cache rather than growing a second rasterizer. 16 fleet manifests already pull a font crate (garasu, mado, tobira, namimado, asobi-text, escriba-render, myaku, hikki, suzuri, kekkai, koyomi, kagi, shashin, nami-core, caixa-bevy-ui, caixa-bevy-text — verified 2026-08-20).
- **mado's `float/` is 1998 lines of GPU-free window-interaction algebra** — but read the types before budgeting it: `geom` (`Rect`/`Edge`/`Corner`/`RectProvenance`) is what a *tiling* consumer needs; the snap-zone system, the drag/resize FSM and the click-to-raise z-stack are **floating** machinery. See P6 for the honest scope.
- **`saihai` has 284 authored `defaction` rows and no backend** (verified).

**What is NOT leverage, and was previously written as if it were.** "38 smithay delegates are compiled in and unwired" is a null fact — an unused macro costs nothing and buys nothing. "Six of nine capabilities are a `delegate_x!` plus a handler impl" collapses a variable cost into a constant: `SessionLockHandler` needs per-output lock surfaces, blanking and crash semantics; `DataControlHandler` needs the whole selection plumbing. **Owed measurement:** per-capability handler line counts taken from anvil/cosmic-comp, held to the same evidence standard as the niri figures below. Until that exists, budget each as unknown, not as small.

**Convergence finding, and it is the strong kind.** niri and cosmic-comp never coordinated, both build on smithay, and both independently hand-wrote **exactly the same four things**: output management, screencopy, foreign-toplevel *management*, and workspaces. That is the smithay boundary being forced by the problem. Budget all four as real projects (niri: 34,300 B and 23,072 B for the first two) — and **all four now have phases** (P9, P10).

**Duplication finding, ruled through the type check.** mado's `float` and ayatsuri's `logic/` are two independent window-interaction substrates with no shared ancestor. mado's snap is a zone **system**; ayatsuri's is two functions. **Same goal, different shapes** → per `theory/CONVERGENT-EVIDENCE.md`, *write the rule down*, do not force one type. Record ayatsuri's bevy-typed macOS shape as a deliberate non-merge.

**Second duplication finding, and this one IS same-shape.** garasu owns `font_cache.rs` (persistent fontdb cache) + `shape_cache.rs` + `text.rs`; asobi-text and escriba-render are independent text consumers. Three consumers, one shape → the font-discovery + shaping-cache layer is a **leaf crate owned by none of them**, with garasu as its second consumer at extraction. That is P7's real deliverable, and it is a bigger fleet win than the drawer.

---

## IV. Names — corpus-checked, not reasoned

The naming law's corpus check **fails silently**. Checked 2026-08-20 against 2441 fleet crate `name =` entries and the pleme-io repo directory list.

| name | gloss | check |
|---|---|---|
| **`egaku-nuri`** | egaku's pixel drawer, over nuri | convention, not a coinage — `egaku-term` is the precedent. Zero naming risk |
| **`kukaku`** (区画) | a parcel — a space partitioned into disjoint plots | **TAKEN — by us, 2026-08-20.** Name checked free in the 1144-repo corpus and on the crates.io sparse index (404, calibrated against `serde`=200) before use. Lives at `pleme-io/tear/kukaku` as a workspace sibling, **not** its own repo: it is consumed by tear AND omoya, so it belongs to neither, and tear's workspace is where its publishing machinery already is. **NOT on crates.io** — tear's Actions are declared off (`actions_enabled: false` in org.yaml), so AUTO-RELEASE cannot publish it and omoya consumes it rev-pinned over git. `pending-kukaku-publish` |
| **`noki`** (軒) | eaves | **free** (0/2441, no repo) — but the **gloss check fails**: eaves encode *top*, and a status bar is routinely anchored bottom, with layer-shell offering four anchors. **PROPOSED; ratify through `/naming`** with the anchor objection on the record. Do not create the crate before then |
| **`zaseki`** (座席) | seat — the owed logind leaf crate | **free**; **ratify through `/naming`** — metaphor-family placement is a naming decision |
| the font/shaping leaf crate | discovery + shaping cache + CPU glyph raster cache | **unnamed on purpose.** `mojiban` (文字盤) is taken and is *rich text* (markdown/highlight/spans), not fonts. Route through `/naming` at P7; do not mint here |

**Two corpus results worth recording because they are the trap:** **`tatami` (畳) is TAKEN** — repo *and* crate — and is the obvious wrong pick for a tiling layout. **`madori` (間取り, floor plan) is TAKEN** by the winit/wgpu client-app framework: the best word for "layout" is spent on something that is not layout. `genkan` (玄関) is an empty repo directory — name claimed, unpopulated; find out what it owns before extending the house family.

**No new name for:** lock (a `mukae` mode), notifications (`shirase`), launcher (`tobira`), clipboard (`hasami`), keys (`awase`), theme (`irodori`/`ishou`/`pente`), introspection (`kanshou`), verbs (`saihai`).

---

## V. The typed destination — one net-new domain, one net-new backend, one authoring face

**`(defkukaku …)` — the net-new triplet.**
- Rust border: `#[derive(TataraDomain)]` over `Kukaku { strategy, gaps, focus_policy, exclusive }`, `Parcel(WindowId, Rect)`, `Strategy::{ScrollingColumns, Stack, Single}`.
- Lisp surface: `(defkukaku plo-seat :strategy scrolling-columns :gap 4 :focus follows-map)`.
- Interpreter, **with state made explicit** — scrolling columns is stateful by construction (scroll offset, per-column width, insertion index relative to focus), and hiding that in `Kukaku` breaks purity while hiding it in the `Env` makes the proptest quantify over nothing:

  `apply<E: LayoutEnv>(&Kukaku, &LayoutState, &[Toplevel]) -> (Vec<Parcel>, LayoutState)`

  `LayoutState` is a **proptest input**, so disjointness is quantified over states, not just over window counts.
- **Why scrolling columns:** opening a fourth window does not resize the other three. sway's splith/splitv/tabbed/stacked tree is more expressive and materially more code. Hyprland and cosmic-comp both keep a floating escape hatch, and **that hatch is exactly the pointer-grab machinery tiling lets us not build** — defer it, do not design it in.

**`(defbackend omoya …)` in saihai — the net-new backend row.** The seam where a keybinding and an MCP tool become the same call.

**`(defnoki …)`** — an authoring face over ayatsuri's already-typed bar model, once the name is ratified.

**Reused, not minted:** `(deftheme …)` from `pente`; `awase::BindingMap` for keys (**assigned to P6, with a hardcoded table at P1**); `shikumi::TieredConfig` + the HM/NixOS/Darwin module trio for every component's config; `kotae`'s four-arm answer shape for every MCP leaf.

**The invariants of §I, as types:**

1. The scanout mapping type is **private to one module whose entire public surface is `Present(&[Rect])`** — not merely missing an `as_slice` method. Sealed by a trybuild test that fails **construction of any reader** plus a forbidden-symbol CI lint (`as_ptr`, `as_mut_ptr`, `Deref`, `Index`, `from_raw_parts`, `to_vec`) scoped to that path. A name-based absence check would leave five other ways in.
2. The `zwp_linux_dmabuf_v1` global binds **only** from a `RendererCaps::ZeroCopyImport` value, constructible only by a zero-copy importer. "Advertise dmabuf, then CPU-copy it" has no code path. *(The nix repo's derived-`readOnly`-option doctrine, in Rust: which globals we advertise is a **function** of the renderer's typed capability, never an independent toggle. The defect happened because it was a toggle.)*
3. `ClientOutbox` owns `flush_clients`, flushed at dispatch-completion. The render callback holds no outbox handle.

---

## VI. MCP observability — a design input, with P0 descoped to what earns its keep

**(a) Every phase's done-predicate is an MCP query.** A capability that cannot be asserted through a leaf is not finished, and the leaf ships in the same commit.

**(b) The verb vocabulary has two faces and one body.** `saihai`'s catalog is the keymap's target *and* the MCP write tools' target. That is why the keybinding path (P6) and `defbackend omoya` (P11) are the same story told twice.

**(c) The argument for this design is how its own root cause was found.** The finding that `flush_clients()` runs only inside the repaint timer took reading three files across two repos plus a vendored crate. **No counter in omoya today could have shown it** — `windows` is a bare `space.elements().count()` and `frames` counts timer fires *including failed frames* (`drm.rs:625-627` logs `frame failed` and continues; `:649` increments unconditionally). A compositor failing every frame reports a rising `frames`.

### The leaves — P0 ships four, the rest arrive with their consumers

Front-loading nine leaves is a multi-week instrumentation build undertaken while the seat runs at 1.8 fps. **P0 = `scene`, `focus`, `frame_timeline`, `input_latency`.** The rest land in the phase that needs them.

| leaf | phase | what it publishes |
|---|---|---|
| `scene` | **P0** | per window `{id, app_id, title, rect, z, focused, activated, mapped, decoration, last_commit_seq, last_commit_at_us, buffer{w,h,format,kind}}` |
| `focus` | **P0** | current keyboard/pointer focus + a bounded ring of transitions `{at_us, from, to, cause}` — focus is set at **two** sites (`handlers.rs:127-133`, `input.rs:187-196`) and neither records which won |
| `frame_timeline` | **P0** | ring of 256: `{seq, timer_fired_at_us, compose_us, elements_drawn, pixels_painted, flip_submitted_at_us, flip_completed_at_us, result: ok\|err(reason)}` + p50/p90/p99/max. mado's `frame_perf` is last-sample-only — copy the plumbing, not the statistic |
| `input_latency` | **P0** | **two in-process legs only**: `read→dispatch`, `dispatch→flush`. See the predicate below for why the other two are not P0 |
| `pixel_at(x,y)` | **P0** | the "vs what is" half — a one-pixel question answered in one pixel |
| `damage` | P2 | rects **computed** vs rects painted |
| `clients` | P2 | per `wl_client`: pid, exe, protocols bound, **bytes buffered outbound, age of last flush** |
| `devices` | P6 | evdev devices by stable `input_id()`, caps, the keymap actually loaded |
| `journal` | P6 | bounded ring of typed compositor events |
| `capture_region` | P10 | needs screencopy; see the legality note below |

WRITE: `send_keys`/`send_chord`, `pointer_move`/`pointer_click`, `window_place`, `spawn`, `set_pacing`, `vt_switch` (**which does not exist today — see the GO/NO-GO box; do not list it as available until P1 lands it**), `lock`.

`set_pacing` is what makes this a desktop *shaped around* observation: timer↔vblank and damage-on/off become live knobs, so an A/B is a tool call, not a rebuild.

### Authorization — Observe-only at P0, because `Rehearsed` depends on capture

`introspect.rs:18-41` already refuses mutation and names the prerequisites. Two fleet precedents compose:

- From **breathe**: the authored/resolved split. `WriteIntent{Observe, Write{authorized_by}, Frozen}` vs the resolved `EffectiveGate{Shadow{reason}, Live{witness}}`. Two properties stolen exactly: **an accidental hold is distinguishable from an authored one**, and **`Write` carries its author in the type**.
- From **banken**: the three legality classes. Honestly: `ActionLegality` lives *inside* banken and is not a consumable library. Extract or mirror it; do not pretend it is importable.

**`Rehearsed` is deferred to P11, and the draft's schedule for it was circular.** "Composite off-screen, let the agent read the capture, then confirm" requires working screencopy — which this plan itself budgets as a 23 KB project at P10 — *and* requires speculative scene evaluation (double-buffered scene state), which is a feature, not a reframing of banken's "git mutates, never the cluster". So:

| class | tools | phase | rule |
|---|---|---|---|
| `Observe` | all leaves | **P0** | always allowed |
| `BreakGlass{witness, runbook, unwitnessed: bool}` | `vt_switch`, `lock`, `set_pacing`, mode change | **P1** | physical-presence class. **`unwitnessed: true` is stamped and published whenever no capture backs the write** — the honest state from P1 through P10 |
| `Rehearsed{authorized_by}` | `window_place`, `spawn`, `send_keys` | **P11** | applied to the scene and composited to an off-screen capture first; the agent reads the capture, then confirms |

**Dead-man revert, classified by revert-direction, not by legality class.** "Auto-revert after N seconds unless confirmed" applied to `lock` **unlocks the screen**; applied to `vt_switch` it bounces the operator's VT back. And a timer living inside the compositor cannot fire in exactly the class it was written for — a wedged compositor (SEAT-PLAN §0's ~19 minutes unreachable). Therefore:

- **One-way writes** (`lock`, `vt_switch`, mode change) **never auto-revert.** Their recovery is the VT, which is why the GO/NO-GO box is at the top of this document.
- **Reversible writes** (`window_place`, `set_pacing`, `spawn`) auto-revert, held by a **separate supervisor process** carrying the last-known-good state — not by the compositor whose wedge is the failure mode.

**Honest limit on the transport:** kanshou's socket is per-`(app, pid)` under the runtime dir with **no principal concept and no authentication**. `authorized_by` is a *claim carried in the query*, not an identity. Graded `only-mitigated (C2)`.

**Cross-process correlation — DESIGN, and it is why `input_latency` ships two legs.** `flush→client_commit` crosses into another process; `commit→flip` crosses back. **No correlation id exists in any of `tear`, `mado` or `omoya` today**, and minting one needs a field on the Wayland-side event with no protocol home. P0 ships the two in-process legs — which is where the measured defect actually lives — plus a separately-labelled cross-process *estimate* (monotonic seq + timestamp comparison across three sockets: **correlation, not causation**).

### Three composition rules, each with a receipt

1. **Do not compute histograms in the sidecar.** The loop *publishes*, the sidecar *reads* — a sidecar that locks `Omoya` stutters the thing it observes. Fixed-size POD ring written by the loop, read lock-free. Not `Mutex<Vec<Sample>>`.
2. **Every leaf ships a ROUND-TRIP test, not a presence check.** The `owed_vt_switches` defect is documented in-tree at `drm.rs:672-691`: *"There are two fields with this name"* — a writer **existed** and wrote the wrong one. A gate whose red-run is "stub the writer out" would have passed that defect, and `every_schema_leaf_answers` (asserting `v["frames"] == 1`) is the test that already existed and did not catch it. The gate must **drive the writer's real input** (a synthetic VT request, a synthetic frame) and assert the **published** field moves.
3. **The write queue must wake the loop.** Copy mado's `InjectedActions` (`Arc<Mutex<VecDeque>>` + `sink_attached`) **with the modification omoya's own header names**: mado drains per GUI frame under `ControlFlow::Poll`; an idle compositor is not rendering. Pair the queue with `calloop::ping::make_ping` so the *enqueue* wakes the loop. Keep `sink_attached` — it is what stops `queued: true` being reported into a void.

---

## VII. Phases — ordered by leverage, with what-unblocks-what named

Every done-predicate is a **measurement**. No phase closes on a claim.

### P0 — SENSOR (minimal) · four leaves, Observe-only

**Deliver.** Carry the evdev µs stamp forward (`evdev_backend.rs:642-647` already produces `Base.time` and it dies there — PRESENT-BUT-UNWIRED, which materially lowers the cost); `scene`, `focus`, `frame_timeline`, `input_latency` (two legs), `pixel_at`; `omoya mcp` as a subcommand of the same binary on `kanshou::mcp::forward_status`; `blackmatter.components.omoya.mcp.serverEntry`; `SeatLegality` with `Observe` only; one **round-trip** test per leaf.

**Explicitly excluded:** damage regions (a *performance* fix re-scoped S→L — `drm.rs:273 prepare()` has zero callers), `clients`, `devices`, `journal`, `capture`, `Rehearsed`.

**Done-predicate (measurement).**
- `input_latency` returns **two** legs with p50/p90/p99/max over ≥256 samples on plo, **and** an injected 50 ms sleep in each stage moves *exactly that leg and no other* — two red-runs. Four plausible non-zero numbers that attribute nothing is the failure shape this predicate exists to refuse.
- The two legs **sum to** an independently measured `evdev_read → dispatch_complete` within a stated tolerance.
- `frame_timeline` reports `result: err` as a count **separate from** `frames`; **red-run:** force a frame failure, confirm the error count rises while `frames` does not.
- CI gate `every_leaf_round_trips` goes **red** when a writer writes a same-named sibling field rather than the published one — red-run reproduced against the `owed_vt_switches` shape specifically, not against a stubbed writer.
- A `BreakGlass` write with `session_active == 0` returns `Shadow{NotReady}` **or** `unwitnessed: true`, never a silent no-op success.

**Unblocks.** Every subsequent done-predicate.

---

### P1 — THE SEAT BECOMES USABLE · the week that changes daily life

This phase merges what were separately "the latency edits" and a set of items the previous draft scheduled nowhere at all. **It is the only phase that must be done before plo is anyone's primary seat.**

**Measured baseline, recorded as the denominator** (plo, 2026-08-20, DP 1920×1080@60, one mado client at 1280×800): **1.80 fps, 535.7 ms frame period, 99.2% userspace**, ~1.1–1.6 s keystroke→pixel. 89% of the frame is two code paths CPU-**reading** NVIDIA GPU memory at **8.3 MB/s** — a measured **1083×** slowdown versus RAM on the *same* buffer (996.30 ms vs 0.92 ms for 8.29 MB), while the *write* direction is only 3.2× slower. The rasterizer is not slow: a full 1920×1080 clear plus a 1280×800 blit into the real scanout measures **4.87 ms**.

**★ The baseline's provenance must be re-taken before it is attributed.** The measured process was named `rust_omoya-0.1.19`, and `5b93d5e` (v0.1.19) → `427cb78` → `2af277a` (dmabuf advertise) → `ef6cf3d` (v0.1.20) means a binary carrying that version string was built from a window that **contains** the dmabuf commit. The version string cannot tell you whether the measured process had the dmabuf import path at all — and the 322 ms attribution rests on exactly that. **Re-baseline against a store path (`nix path-info`), not a version string, and confirm from `WAYLAND_DEBUG` that the client actually bound `zwp_linux_dmabuf_v1`, before attributing a millisecond to it.**

**Deliver, in this order.**

1. **Deploy HEAD with `2af277a` reverted.** `c0e54df` (pace from the deadline — `TimeoutAction::ToDuration` resolved as `Instant::now() + d` *after* the callback returned, so the real period was `interval + render_time`) and `eec66b8` (one flip in flight) are measured, landed, and not deployed. Note honestly: `eec66b8`'s own subject says the pacing change *"exposed an EBUSY the slack was hiding"* — this is a change to the flip path that has never run on plo, not free relief. "Deploy HEAD" and "revert `2af277a`" are the same branch: state the intent as **HEAD-minus-`2af277a`**.

2. **The dmabuf gate — prove the fallback before taking it.** The claim "mado falls back to `wl_shm`, a memfd in ordinary RAM" is **unverified and load-bearing for 60% of the headline number**. mado is `winit = "0.30"` + `wgpu = "25"` (verified 2026-08-20) with no buffer-protocol code of its own — the fallback is chosen by the Vulkan WSI on plo's **NVIDIA** driver, which negotiates through `wl_drm`/linux-dmabuf and has no `wl_shm` hardware swapchain path. Two failure modes, both of which read as success on a naive predicate: swapchain creation fails and the only client on the seat does not start, or it silently drops to **llvmpipe** — "recovering 322 ms" by making the client software-rendered.
   **Procedure:** withhold the global on a scratch instance, run mado under `WAYLAND_DEBUG=1`, capture the actual bind list, **and assert through mado's own `caps` surface that the adapter is still the NVIDIA one**. Keep an ssh rollback path open. **If the shm path does not exist, this edit is unavailable and P1's latency recovery is ~134 ms, not ~456 ms** — which changes this phase's numeric predicate, and that is the honest outcome, not a failure.
   Ship the gate as a **runtime flag flippable over ssh**, not a source revert. *(Invariant it stands in for: 2 — the global is a derived capability of the renderer.)*

3. **Opacity, declaratively.** `a >= 1.0` takes the copy arm at `nuri/src/lib.rs:218` and every one of mado's 1,024,000 destination reads disappears (**−134 ms**). **But `~/.config/mado/mado.yaml` is an untracked machine-local file** invisible to this repo's declarative doctrine and liable to be reverted by the next HM rebuild — land it as a typed HM setting. And it is **client-side**: any third-party translucent client restores all 134 ms. It does not generalize; P2 is where it does. *(Invariant: 1 — the blend destination is RAM.)*

4. **Flush at dispatch-completion** — flush the client socket at the end of `process_input_event`, or give `main.rs:702`'s empty post-dispatch callback a flush. Removes **a whole frame period** from the keystroke→client leg. *(Invariant: 3 — flush ownership.)*

5. **A hardcoded chord table.** Today the only way a program starts is `omoya -- <cmd>` (`main.rs:48-50`, spawned once at `:686-698`). Ship spawn / close / focus-next / focus-prev / VT / lock as a fixed table now; `awase::BindingMap` replaces it at P6. **Without this you cannot open a second window, which means you cannot exercise, let alone measure, anything else in this plan.**

6. **A cursor sprite** — `handlers.rs:253`'s empty `cursor_image` is why the pointer is invisible. xcursor theme load + `wp_cursor_shape_v1`.

7. **`change_vt` actually called, and the evdev-grab question settled** — see the GO/NO-GO box. One change closes both halves: call `change_vt` on the recognised chord, and take an explicit, written decision on `EVIOCGRAB`/`KDSKBMODE` with the leak consequence stated.

8. **A fallback session at the greeter.** `services.greetd.settings.default_session` is a single command; the parked Hyprland profile is a `nixos-rebuild` away, not a login-menu pick. On one machine with no fallback this is non-negotiable and it costs one attrset.

9. **A supervision policy.** A compositor crash takes the Wayland socket and therefore every client. The one genuine existing mitigation is that **`tear` owns the ptys**, so shell state survives a mado death — nothing survives an omoya death. Ship: a restart policy, a recovery path that works from inside the session, and a written statement of what is lost.

**Done-predicate (measurement).** Read from P0's leaves on plo:
- `frame_timeline.compose_us` p50 ≤ **20 ms** *with edit 2 available*, or ≤ **150 ms** without it — **state which branch was taken and why**, with the re-taken baseline as the denominator in the same report.
- `input_latency` (two legs) p99 ≤ **250 ms**.
- A chord opens a second window: `scene` shows 2 entries. A chord closes it. A chord moves focus: `focus` records the transition with a `cause`.
- `pixel_at` at the pointer position returns cursor pixels, not client pixels.
- **The VT round-trip:** a marker string typed into the seat, then Ctrl+Alt+F2, then read back from the tty — with the result written down either way.
- The greeter offers two sessions and the fallback one starts.

**★ Sequencing honesty.** P1's edits are hours of work and P0 is weeks; P1 **will** land first. Say so: the relief is labelled **`unattested`** until P0 attests it retroactively. It is not "concurrent" — it is un-instrumented, and a plan that forbids closing on a claim must not close this one on "it feels faster".

---

### P2 — THE FRAME BECOMES CORRECT · write-only scanout + damage + vblank

Moved ahead of layout. The previously-stated dependency (disjoint parcels make damage "simpler and provably correct") is a **convenience, not a requirement** — every production compositor tracks damage over overlapping surfaces — and vblank pacing needs nothing from layout at all. All three deliverables rewrite the same file (`scanout.rs`), so splitting them means doing that rewrite twice.

**Deliver.** `ShadowSurface` — nuri composites into ordinary RAM; the only device-touching op is `Present(&[Rect])`, on a module-private mapping type (§V.1). `OutputDamageTracker` (a wire) plus buffer-age bookkeeping — `nuri` already clips, so this is bookkeeping, not rasterizer work. Vblank from `DrmDeviceNotifier` — a build, because omoya deliberately dropped `DrmCompositor` to avoid linking gbm (`drm.rs:445-455`); `pending-omoya-vblank` is already written at `drm.rs:477-484`. Fold in `wp-presentation-time` (a wire) — it is what makes the phase measurable rather than argued. Add the cheap globals while the file is open: `wp_viewporter`, `xdg_activation`, `zwp_idle_inhibit`, `zwp_relative_pointer`/`pointer_constraints`. Ship the `damage` and `clients` leaves here.

**★ What this phase is actually for — the perf framing was wrong and would have got it cut.** Plan-measured: today's fill+blit into the real scanout is 4.87 ms; the `ShadowSurface` floor is 0.77 + 0.46 + 2.94 ≈ **4.17 ms**. The 134 ms is already gone once opacity is 1.0. **`ShadowSurface` buys ~0.7 ms of latency.** What it buys that matters is the *invariant* (write-only by type), free translucency, and free read-back. **Damage** is what rescues typing. And state the ceiling: the same shape at 3840×2160 is ≈16.7 ms of single-threaded CPU composite — the entire 60 Hz budget, write-leg-dominated. Damage tracking rescues typing; it does not rescue video or animation.

**Done-predicate (measurement).**
- `compose_us` p99 ≤ **8 ms** at 1920×1080 **with the translucent arm active** (`default_opacity: 0.98` restored) — this is the point: it proves the fix is structural and not the P1 config workaround.
- **`computed_px` for a single-cell terminal edit is < 5 000 px**, and `painted_px ≤ computed_px`. *(The previous predicate — `painted/computed == 1.0` — is satisfied by the null implementation: compute the whole framebuffer, paint the whole framebuffer, ratio 1.0, seat unchanged. The number that can lie is the computed one, so the bound goes there.)*
- `flip_completed_at_us` deltas cluster at **16.67 ms ± 1 ms over 600 frames** — proving vblank pacing, not a timer.
- A trybuild test fails on **construction of any reader** of the scanout mapping (not on the absence of one method name), and the forbidden-symbol lint over that module path is red-run.
- Binding the dmabuf global without a `ZeroCopyImport` value is a compile error (red-run recorded).

**Unblocks.** A 60 Hz seat, and every "does it feel right" judgement anyone makes about anything downstream.

---

### P3 — CLIPBOARD, three planes native · minute five of daily use

Copying a URL out of a browser into a terminal is the fifth thing anyone does. `primary_selection` + `wlr_data_control` + `ext_data_control` — all three present in smithay, all three unwired. Wire `security-context` **alongside, not after**: a sandboxed client must be distinguishable from a trusted one *before* data-control exists.

`hikidashi` (231 lines, **zero consumers fleet-wide**, verified 2026-08-20) retires by ★★ MODULARIZE, DON'T DELETE — a typed `enable = false` plus a CI gate that no manifest depends on it.

**Done-predicate (measurement).** `wl-paste --watch` (probe) receives a copy from mado; the content **survives the source process exiting**; middle-click paste works; `cargo tree -i arboard` over the **seat workspace** returns nothing on Linux; `hikidashi`'s retirement flag is typed, its code still builds, and the no-dependent gate is red-run.

---

### P4 — LOCK + IDLE + DPMS + SLEEP/RESUME · hour one of daily use

**★ Corrected dependency, and it is what let this phase move four places earlier: lock does NOT need layer-shell.** `ext-session-lock-v1` supplies its own per-output `ext_session_lock_surface_v1`; smithay ships `session_lock/{mod,lock,surface}.rs` complete. The draft's claim that there is "no surface for a lock UI to draw on" without layer-shell was wrong. Lock needs a *paint face*, and an M0 face is a solid fill plus password dots — which `nuri` can draw with no text stack at all.

**Deliver.** `delegate_session_lock` + `mukae --mode lock` as the session-lock client + `idle-notify` + `idle-inhibit` (wires) + a typed idle policy + DPMS + logind `PrepareForSleep` (device pause/resume is already wired at `logind.rs:309-340`; sleep is not, and an unmodeset resume is untested).

**No second credential implementation.** `mukae-native::verify_user` already does real shadow verification.

**Done-predicate (measurement).**
- `scene.mode == Locked`, and `pixel_at` **anywhere** outside the lock surface returns the lock background — no client pixels leak.
- `send_keys` of a wrong password leaves `mode == Locked`.
- **Red-run:** kill the lock client; the session stays locked (session-lock's crash semantics), verified by `pixel_at`.
- Idle → DPMS off after the declared timeout; a keypress restores modeset within 1 s.
- Suspend and resume: the seat returns to a modeset output with `scene` intact, or the failure is captured and stated.
- The lock crate calls `mukae_native::verify_user` and constructs no `AuthProof` of its own — sealed by the type (§VIII), *not* by a grep for `verify`/`crypt`, which a crate calling a helper defeats.

---

### P5 — LAYER-SHELL · the single highest-leverage wire in the repo

`delegate_layer_shell` + `smithay::desktop::LayerMap`. Exclusive zones become an input to `kukaku`'s parcel space **by derivation**.

**Why it is the leverage gate.** It is the protocol that lets *other people's programs — and ours as separate processes* — be the desktop. Without it there is no bar, no launcher, no notifications and no wallpaper, and no amount of work inside omoya substitutes, because the ecosystem is outside. (Mutter is the one comparator without it — which is exactly why waybar cannot run on GNOME.)

**Done-predicate (measurement).** An unmodified third-party layer-shell client run **once as a conformance probe** (`swaybg`, then `wofi`):
- maps at each of the 4 layers, **and `pixel_at` where two layer surfaces overlap returns the higher layer's colour** — acceptance is not ordering, and a compositor that composites overlay *under* background passes the acceptance-only version of this test;
- its exclusive zone is subtracted: the tiled parcel shrinks by **exactly** the zone height and `pixel_at` on the reserved strip returns the client's colour;
- **`keyboard-interactivity` is honoured**: a `send_keys` witness proves an `exclusive` surface takes the keyboard and an `on-demand` one does not. This is input-routing policy, not a delegate, and it is what makes a launcher usable.
The probes are deleted afterward; nothing is vendored.

---

### P6 — `kukaku` + decorations + the full toplevel surface + the key path

**Deliver.**

- **Extract `geom` only** — `Rect`/`Edge`/`Corner`/`RectProvenance` over `egaku::Rect` — as `kukaku`, owned by neither repo. **The snap-zone system, the drag/resize FSM and the click-to-raise z-stack stay in mado.** They are *floating* machinery, and this phase chooses tiling precisely to avoid building floating; "1998 lines already exist" is not leverage when the consumer needs ~200 of them. If floating ever lands, the rest is extracted then, on a real second consumer.
- **mado's migration onto `kukaku::geom` is an explicit deliverable with a named owner and a behaviour-parity gate** — not an implication of the word "extraction". An extraction with one consumer is a rename.
- The `(defkukaku …)` scrolling-columns spec + `apply<E: LayoutEnv>(&Kukaku, &LayoutState, &[Toplevel])`. omoya calls it, replacing `map_element(w, (0,0))`, and `Space` is wrapped in a newtype whose public surface omits `map_element`.
- **`xdg-decoration` + `kde-decoration` in the SAME change** — the seam is already open at `handlers.rs:130-134`. If you do not advertise the global, a GTK client assumes CSD and draws a titlebar, rounded corners and a shadow **inside** your tile, and the layout looks broken when it is not.
- **The rest of the toplevel request surface, because a browser needs it**: `fullscreen_request`/`maximize_request` (absent today — F11 and fullscreen video do nothing) and **popup grabs** (`fn grab(&mut self, …) {}` at `handlers.rs:169` is a no-op, so menus do not dismiss on click-outside and do not take keyboard focus).
- **The key path: keysym → `awase::BindingMap`/`KeyMode`/`Condition`/`Banzuke`/`KeyRepeatGate` → a saihai `Action`.** This is the phase where there is finally something to bind, and without it the destination's "two faces, one body" has exactly one face — an agent can drive the seat and a human cannot. The hardcoded P1 table is deleted here.
- Ship `devices` and `journal`.

**Why tiling is the choice, not the lesser choice.** `move_request`/`resize_request` are empty bodies; sway and niri also decline client geometry requests for tiled windows — the protocol permits it. **Under tiling those two empty functions are correct; under floating they are bugs.** Floating additionally requires pointer-grab impls for move and resize (the largest single chunk of smithay's anvil state), z-order and raise-on-click, position persistence, resize-edge hit-testing with per-edge cursor shapes, and min/max/aspect-hint honouring.

**Done-predicate (measurement).**
- Headless proptest with `LayoutState` **as an input**: for 1..=16 windows across generated states, parcels are pairwise-disjoint and their union == output minus exclusive zones. **10 000 cases green**, plus a red-run against a deliberately overlapping strategy.
- On plo via MCP: `scene` shows N windows with disjoint rects and **exactly one** `focused: true`, for N ∈ {1,2,3,5} — **and each parcel's interior `pixel_at` returns content distinct from every other parcel's**, with a red-run against `map_element(w,(0,0))` restored proving the predicate goes red. A model-only assertion would pass over a stack of overlapping windows, which is the reported-vs-painted split this whole plan exists to close.
- `scene[].decoration == ServerSide` for a GTK client, and `pixel_at` on the tile's top row returns client content — not a titlebar.
- **The Firefox hour.** An unmodified Firefox: scroll, open the URL dropdown, open a right-click menu **and dismiss it by clicking outside**, upload a file, play a video **fullscreen via F11**. Any failure is a named, filed gap — not a footnote.
- `send_keys` of a bound chord executes the mapped saihai action and the effect is witnessed in `scene`.

---

### P7 — the fleet text stack + `egaku-nuri` · runs in PARALLEL from P2 onward

**★ Re-scoped: this is not "a drawer", and it is not "a second rasterizer" either.** `nuri` is 485 lines in one file with **zero** `glyph`/`font` symbols (verified 2026-08-20), so the draft's "the only thing missing is a drawer that paints pixels" was wrong — what is missing is font discovery, shaping, rasterization and a glyph cache. But the fleet already speaks that wire: `garasu/src/text.rs` drives `FontSystem`/`Buffer`/`SwashCache` and documents the pipeline as *"cosmic-text resolve → swash raster → glyphon"*, and **swash rasterizes to CPU bitmaps that glyphon merely uploads**. So `egaku-nuri` reuses the same shaper and blits the same bitmaps into a RAM framebuffer.

**Deliver.**
1. **The font/shaping leaf crate** (name deferred to `/naming`): fontdb-backed discovery + persistent cache + shaping cache + a CPU glyph-bitmap cache. **Extracted from garasu's `font_cache.rs`/`shape_cache.rs`, owned by neither**, with garasu as its second consumer at birth and `asobi-text`/`escriba-render` as the third and fourth. This is the larger fleet win of the two.
2. **`egaku-nuri`** — the drawer, over that crate + `nuri`.

Value: every shipped egaku widget becomes paintable (a) in-compositor with no GPU and no client, and (b) **inside a `wl_shm` client** — which is precisely what a bar, a launcher, a lock face and a notification banner are. Today the only way to build one is garasu/wgpu, which is why `tobira` positions itself with `WindowLevel` + absolute `PhysicalPosition`, a model `xdg_toplevel` cannot honour.

**Done-predicate (measurement).** **Not** a text-extraction oracle over egaku's entire corpus — that oracle is itself a project. Instead: a **golden screenshot hash** for a fixed widget tree at a fixed size, plus ~12 targeted glyph assertions (baseline, advance, kerning pair, emoji colour bitmap, CJK fallback, ellipsis), plus one in-compositor `pixel_at` against a known token colour. Plus: garasu builds and passes its existing tests against the extracted crate, byte-parity on its shaped output.

**Unblocks.** `noki`, the full lock face, `tobira`'s surface, `shirase`'s banner — all four without garasu/wgpu.

---

### P8 — `noki` + `tobira`'s surface + `shirase`'s wire · needs P5 + P7

`noki` = ayatsuri's status-bar model + `logic/bar_layout.rs` (objc2 hit-count **0** across `builtins`, `components`, `config`, `mod`, `theme`; only `render.rs` and `window.rs` are Cocoa) on `egaku-nuri` over layer-shell — **name ratified through `/naming` first** (§IV). `tobira` gains a layer-shell surface and retargets its window actions from `hyprctl dispatch` to omoya's saihai backend. `shirase` gains an `org.freedesktop.Notifications` server and a real banner.

**Done-predicate (measurement).**
- `notify-send` (probe) → `shirase history` shows the record **and** `pixel_at` inside the banner rect is non-background within **100 ms**.
- the bar's clock advances (two `pixel_at` samples 1 s apart differ) while `damage.computed_px` for that frame is **< 20 000** — proving P2 actually bought something.
- `tobira` raises a window by id (`scene[].focused` flips) with **`hyprctl` occurrence count == 0** in the binary.

---

### P9 — WORKSPACES + foreign-toplevel management · the two convergent projects nobody had scheduled

§III's convergence finding names four things niri and cosmic-comp both hand-wrote; the draft scheduled two of them. These are the other two, and **saihai's catalog has already decided they are required**: 40 of its 284 `defaction` rows mention `workspace` (verified 2026-08-20). Without this phase, P11's predicate is unreachable honestly and trivially reachable dishonestly — by writing 284 arms that return `Ok(())`.

**Deliver.** A workspace model in `kukaku` (a parcel space per workspace, one active per output), `ext-workspace` for third-party bars, and `foreign-toplevel-management` so a launcher or bar in another process can enumerate and raise windows.

**Done-predicate (measurement).** A chord moves the focused window to workspace 2 and `scene` reflects it, with `pixel_at` confirming workspace 1 no longer paints it; `wofi`/a foreign-toplevel probe enumerates the same window set `scene` reports, with **zero divergence over 20 mutations**.

---

### P10 — OUTPUTS: management, hotplug, fractional scale, screencopy, portal

Budget as **real projects**, on the convergent evidence: niri hand-wrote 34,300 B for output management and 23,072 B for screencopy; cosmic-comp wrote directories for the same two. Hotplug is a one-line filter widening plus a real device path — `uevent.rs:216` drops everything that is not `input`, and there is a test literally named `a_drm_hotplug_is_not_an_input_hotplug`. Add `wp_fractional_scale` + per-output scale (without it a HiDPI panel renders at 1×) and the xdg-desktop-portal + PipeWire node (without it there is no screen sharing, indefinitely).

**Done-predicate (measurement).** `wlr-randr` (probe) enumerates and changes mode/position on a second head; a DP hotplug produces a `scene.outputs` delta within **2 s**; unplugging the only head is exercised and its behaviour written down; `grim` (probe) captures a region **byte-identical** to `capture_region`'s output for the same rect; a fractional-scale client reports the scale `scene` reports; a screen-share through the portal shows the seat in a third-party consumer.

---

### P11 — `(defbackend omoya)` + `Rehearsed` · the induction

284 authored `defaction` rows get an executor; the keybinding and the MCP tool become the same call. `Rehearsed` legality lands here because it is the first point at which capture exists (P10) — see §VI.

**Done-predicate (measurement).** The catalog compiles against the omoya backend with **zero `UnknownAction`**; a CLOSED-LOOP matrix **fails the build** when a `defaction` has no backend arm (red-run recorded); **every arm carries either a witness query (`scene`/`pixel_at` delta) or an explicit `unwitnessed` mark, and the unwitnessed count is published in the matrix output** — a stub returning `Ok(())` must be visibly a stub, which is the same failure shape as the capture stub and the answering-leaf; **N of 284 executed live and witnessed**, with N **stated, never rounded**.

---

## VIII. REQUIRED DELIVERABLE — the tier-honest ledger

**Closed tier vocabulary:** `truly-unrep` · `parse-time-rejected` · `only-mitigated (C<n>)`. **Status vocabulary:** SHIPPED · SHIPPED-composition · NET-NEW(P<n>) · GUEST · ABSENT-runtime.
**Ceilings:** **C1** — no dependent types, so a coverage/call-graph quantifier terminates at a build-failing test. **C2** — external-world observation, so what a panel showed, what a monitor is, who sent a query and whether a secret left through a foreign sink are outside the type. **C4** — an irreducibly-shared real resource, so contention terminates at a runtime lease, never a private cell.

**Every tier is the tier the row reaches at the phase named. Nothing is claimed as reached today except rows marked SHIPPED. Where two axes grade differently they are separate rows, so the strong axis never launders the weak one.**

<!-- tier-ledger -->

| capability | pleme-io realization | tier |
|---|---|---|
| a CPU read of a device buffer (today: 322 ms/frame, 8.3 MB/s) | NET-NEW(P2): the scanout mapping is private to one module whose public surface is `Present(&[Rect])` | truly-unrep **outside that module**; only-mitigated (C1) inside — terminal is the forbidden-symbol lint over that path |
| dmabuf advertised but consumed by a CPU copy | NET-NEW(P2): the global binds only from `RendererCaps::ZeroCopyImport`, constructible only by a zero-copy importer | truly-unrep |
| read-modify-write of the scanout (today: 134 ms/frame) | NET-NEW(P2): blend destination is the RAM `ShadowSurface` | truly-unrep |
| input delivery coupled to repaint | NET-NEW(P1): `ClientOutbox` owns the flush at dispatch-complete | only-mitigated (C1) — "no render→flush edge" is a call-graph quantifier; terminal is a forbidden-edge lint + the measured `dispatch→flush` p99 |
| keystroke→photon budget met on the panel | NET-NEW(P0): two in-process legs + `pixel_at`, attested per build | only-mitigated (C2) — whether the panel showed it is outside the process; terminal is runtime observation |
| latency attributed to the wrong stage | NET-NEW(P0): per-leg injected-sleep red-runs + a sum-vs-independent-measure check | only-mitigated (C1) — terminal is the four red-runs in CI |
| cross-process latency attribution (`flush→commit→flip`) | **DESIGN** — no correlation id exists in tear, mado or omoya | only-mitigated (C2) — published as a labelled *estimate*; terminal is a minted id with a protocol home |
| no way to open a second window (`omoya -- <cmd>` only) | ABSENT-runtime → NET-NEW(P1) hardcoded chords → NET-NEW(P6) awase `BindingMap` → saihai `Action` | only-mitigated (C1) — terminal is a `send_keys`→`scene` witness per bound chord |
| the pointer is invisible | ABSENT-runtime (`handlers.rs:253` empty) → NET-NEW(P1) sprite + `wp_cursor_shape_v1` | only-mitigated (C2) — terminal is `pixel_at` at the pointer position |
| no VT escape from a wedged seat | ABSENT-runtime — `change_vt` defined ×2, **called 0×** → NET-NEW(P1) | only-mitigated (C2) — terminal is the marker-string VT round-trip, run before plo is a sole seat |
| keystrokes also delivered to the kernel tty | **UNRESOLVED** — no `EVIOCGRAB`/`KDSKBMODE` anywhere → NET-NEW(P1) explicit grab decision | only-mitigated (C2) — a lock-screen password leak until settled; terminal is the tty read-back |
| a compositor crash taking every client | NET-NEW(P1) supervision policy; `tear` already survives a mado death | only-mitigated (C4) — one process owns the socket; terminal is a restart policy + a written loss statement |
| damage computed then discarded | NET-NEW(P2): `Frame` has no paint-everywhere op; every op derives from `&[Rect]` | truly-unrep |
| a repaint wider than the change | NET-NEW(P2) + the `damage` leaf | only-mitigated (C1) — terminal is the **`computed_px` < 5 000** bound for a single-cell edit (a painted/computed *ratio* is satisfied by the null implementation) |
| clipboard content dying with the source process | NET-NEW(P3): `wlr-data-control` + `ext-data-control` manager holds the offer | only-mitigated (C2) — survival depends on a live manager process outside ours; terminal is the `wl-paste`-after-kill probe |
| two clipboard implementations disagreeing | SHIPPED-composition(P3): `hasami` is the sole model; `hikidashi` typed `enable = false`, zero consumers today | only-mitigated (C1) — a config flag is not a type; terminal is a build-failing no-dependent gate |
| the lock dismissed without proof | SHIPPED-spec / **ABSENT-runtime** → NET-NEW(P4): `Locked` typestate, `unlock(AuthProof)` by value | truly-unrep — **in the spec only; runtime `SeatMode` has no `Locked` arm and `lock` is a rejected `--mode`. Do not read this row as live before P4** |
| a second credential implementation | NET-NEW(P4): `AuthProof` is constructible only inside `mukae-native` | truly-unrep — attributed to the constructor, **not** to a `verify`/`crypt` grep, which a crate calling a helper defeats |
| libpam on the credential path | GUEST today (`mukae-host`) → NET-NEW `mukae-native` (yescrypt + sha-crypt, verifying real hashes) | only-mitigated (C2) — blocked on `/etc/shadow` privilege, not crypto; terminal is a privileged helper with an attested boundary |
| password material reaching a log/argv | SHIPPED-composition: `Zeroizing<String>`, no-argv/no-env discipline, mukae masking | only-mitigated (C2) — a foreign sink (journald, a crash dump, **the ungrabbed tty**) is outside the type |
| the screen never blanks / never resumes | ABSENT-runtime → NET-NEW(P4): idle policy + DPMS + `PrepareForSleep` | only-mitigated (C2) — terminal is the suspend/resume observation |
| a bar/lock/launcher overlapping a tile | NET-NEW(P5): kukaku's input space is output-minus-exclusive-zones, derived not subtracted | truly-unrep |
| layer ordering / keyboard-interactivity wrong | NET-NEW(P5) | only-mitigated (C2) — terminal is the overlap `pixel_at` + the `send_keys` interactivity witness |
| a window mapped outside a computed layout | NET-NEW(P6): `Space` wrapped in a newtype whose public surface omits `map_element` | only-mitigated (C1) — `Space::map_element` is smithay's API on smithay's type and cannot be removed; terminal is the newtype + a forbidden-symbol lint |
| layout parcels disjoint and covering | NET-NEW(P6): total layout fn over an explicit `LayoutState` + 10 000-case proptest **with state as an input** | only-mitigated (C1) — terminal is a build-failing proptest |
| the model says disjoint while the screen overlaps | NET-NEW(P6): every parcel assertion carries an interior `pixel_at` | only-mitigated (C2) — terminal is the paired pixel witness + the `map_element` red-run |
| a client titlebar drawn inside a tile | SHIPPED-composition(P6): `xdg-decoration` + `kde-decoration` answer `ServerSide` | only-mitigated (C2) — decoration is a *negotiation*: a client that never binds the manager draws CSD unilaterally, and GTK4 draws shadow/rounded corners regardless |
| menus that do not dismiss, F11 that does nothing | ABSENT-runtime (`grab` no-op; no fullscreen/maximize handler) → NET-NEW(P6) | only-mitigated (C2) — terminal is the Firefox hour |
| a notification bypassing DnD/filter policy | SHIPPED-composition(P8): shirase's filter is the only path to the banner renderer | only-mitigated (C1) — a call-graph quantifier, graded the same as the flush edge |
| a malformed D-Bus notification payload | NET-NEW(P8): typed parse at ingress, deny-unknown | parse-time-rejected |
| a `defaction` naming a capability nothing implements (40 workspace rows) | NET-NEW(P9) workspaces + foreign-toplevel management | only-mitigated (C1) — terminal is the P11 matrix with the unwitnessed count published |
| output/hotplug state matching reality | NET-NEW(P10): DRM uevent → reconciled `outputs` leaf | only-mitigated (C2) — terminal is uevent observation + the 2 s delta gate |
| a HiDPI panel rendered at 1× | ABSENT-runtime → NET-NEW(P10): `wp_fractional_scale` + per-output scale | only-mitigated (C2) |
| an MCP write while the agent cannot see the result | **ABSENT-runtime until P10** (capture does not exist) → NET-NEW(P11) `Rehearsed` | only-mitigated (C2) — until then every write is stamped `unwitnessed: true`; terminal is the capture probe |
| a reversible seat write left un-reverted | NET-NEW(P1): dead-man revert held by a **separate supervisor**, scoped to reversible writes only | only-mitigated (C4) — a wedged compositor cannot revert itself; `lock`/`vt_switch` are one-way and never auto-revert |
| an unattributed seat write | NET-NEW(P0): `Write{authorized_by}` is a tuple variant | only-mitigated (C2) — the kanshou socket carries no authenticated principal, so `authorized_by` is a claim; terminal is the audit journal |
| agent and human writing the seat at once | NET-NEW(P1): break-glass lease | only-mitigated (C4) — one screen, one keyboard; terminal is a runtime lease |
| a leaf that answers while nothing feeds it | NET-NEW(P0): **round-trip** test per leaf — drive the writer's real input, assert the published field moves | only-mitigated (C1) — terminal is a build-failing matrix. **Measured live: `owed_vt_switches` answered `0` forever, and a writer EXISTED (`drm.rs:672-691`) writing a same-named sibling — a presence check would have passed it** |
| fleet visual drift across seat surfaces | SHIPPED-composition: `FleetThemedConfig::from_fleet` + `convergence::Guard` | only-mitigated (C1) — terminal is the Guard drift test |
| two font/shaping stacks in the fleet | NET-NEW(P7): one leaf crate, garasu as second consumer | only-mitigated (C1) — terminal is a no-second-fontdb gate + garasu byte-parity |
| non-US layouts, compose, dead keys, IME | **ABSENT, unscheduled** | `pending-seat-input:` — a stated consequence until it has a phase |
| X11-only clients | **GUEST, excluded by decision** (`SEAT-PLAN.md:389`) | see §IX — a population argument, re-decided deliberately |

---

## IX. The retirement differential — when "we replaced the desktop" becomes true

Not before. The claim is a **green differential**, never an assertion, with **two halves and two denominators** — because one enumeration cannot see both kinds of thing:

1. **Absence — binaries.** `swaybg`, `waybar`, `wofi`, `mako`, `swaylock`, `swayidle`, `wl-clipboard`, `grim`, `slurp`, `wlr-randr` are absent from **`nix path-info -r` over the node's `system.build.toplevel`** — not the flake closure, which contains the conformance probes and would fail this half forever. The probes live in a separate check derivation, named in the predicate as excluded.
2. **Absence — crates.** `arboard` is a statically linked Rust crate that no store-path enumeration will ever find. Its denominator is **`cargo tree -i arboard` over the seat workspace**.
3. **Presence.** Each capability green in the verification matrix, proved by its phase's done-predicate, re-run in CI.

Until all are green, the honest statement is *"we compose N shipped primitives and have rebuilt M of the seat's capabilities"* — with N and M stated. **omoya remains a `resident`, not a citizen, until then**, exactly as its own `CITIZENSHIP.md` grades it.

**Two capabilities will not reach the absence half, and both are decisions, not gaps.**

- **XWayland.** The population is smaller than previously written and more painful. Modern GTK/Qt are Wayland-native; JetBrains ships a Wayland-native JBR (2024.2+); Electron ≥ 20 *can* run native Wayland via ozone but **does not by default** without explicit flags. The real casualties are therefore **Cursor** — a named fleet tool with its own blackmatter module and its own skill — unless it is ozone-flagged and that flag is verified, plus **Steam** and GTK2/Qt4-era applications. "Your editor" is the honest way to state this, not "Electron, Steam, JetBrains".
- **Non-US keyboard layouts and IME**, if the hairetsu limit is permanent. Confirm the cite, then either schedule it or put it here in writing.

---

## X. The Care

**Ceilings we do not cross, named up front.** **C1** bounds every coverage and call-graph invariant here — disjointness, the damage bound, the leaf round-trip gate, the action-backend matrix; the terminal for each is a build-failing test and chasing a compile error past that is wasted effort. **C2** bounds every claim about what the panel showed, what the monitor is, who sent a query, whether a client honoured a negotiation, and whether a secret left through a foreign sink. **C4** bounds seat contention and crash recovery: one screen, one keyboard, one process owning the socket — a runtime lease and a supervisor, never a private cell.

**Corrections that must land with this plan** — each is a live-false statement in a steering surface today:

| surface | says | reality (2026-08-19/20) |
|---|---|---|
| `nix/docs/saihai-desktop-action-spec.md:1441` | "omoya does not exist … no fleet node runs a Wayland compositor" | omoya is `roles.compositor` on plo; `pleme.nixos.mode.environment = "pleme-omoya"` (`nodes/plo/mode.nix:133`) |
| `mukae/README.md` | "M1–M9 … remain design" | `mukae-greeter` ships a `[[bin]]`; `mukae-native` verifies real shadow hashes and registers a logind session. The honest remaining gap is `pam_open_session`/exec — it authenticates and does not seat |
| `pending-nuri-dmabuf-zerocopy` | implies holding the mapping is the fix | **it makes the frame worse** — per-pixel uncached VRAM reads instead of one bulk read. Rewrite the marker so no future agent reaches for it |
| `omoya/docs/SEAT-PLAN.md` | cites `handlers.rs:132-141` for the empty request bodies | drifted; they are at `:157` and `:160` |
| `profiles/nixos-pleme-omoya/default.nix` | *"Recovery is a TTY (ctrl-alt-F2 reaches a getty)"* | `change_vt` has **zero call sites**; `input.rs:65-80` counts the chord and forwards it. Whether the kernel VT still honours it is unresolved (no `EVIOCGRAB`) — **the rollback documented for this machine is not known to work** |
| this plan's own previous draft | "no surface for a lock UI to draw on" without layer-shell | false — `ext-session-lock-v1` supplies its own per-output lock surfaces; lock moved from P6 to P4 on that correction |

**Anti-patterns this plan forbids.**
- Recording P1 as "the latency fix". It is interim edits standing in for three invariants; P2 is where the class dies.
- **Reverting the dmabuf advertisement without first proving the fallback**, and accepting a green predicate that was bought by an llvmpipe demotion.
- Closing P1 on "it feels faster". It closes `unattested` until P0 reads it.
- **A presence check where a round-trip is owed.** A writer existed for `owed_vt_switches`; it wrote the wrong field, and the leaf's *answering* is what made it look healthy.
- **A predicate the null implementation satisfies.** `painted/computed == 1.0` was one. Bound the number that can lie.
- **A predicate that reads the model where the screen is the question.** Every layout assertion carries a pixel witness and a red-run.
- Putting mutation behind the read schema. `capture` already lives as an `args`-bearing *write* in a file whose header says every leaf is a READ; once `BreakGlass` lands it must be a distinct arm carrying `SeatLegality`.
- Merging mado's `float` with ayatsuri's `logic/`. Same goal, different shapes.
- **Calling a one-consumer extraction an extraction.** mado's migration onto `kukaku::geom` is a deliverable with an owner and a parity gate, or the extraction is a rename.
- Designing the floating escape hatch "for later". It is the pointer-grab machinery tiling exists to avoid.
- Auto-reverting a one-way write. Dead-man revert on `lock` unlocks the screen.

**Unverified, and labelled as such.**
- **The `wl_shm` fallback** (load-bearing for 60% of the headline number). mado is winit/wgpu with no buffer-protocol code; the fallback is the NVIDIA Vulkan WSI's decision, not mado's.
- **The baseline's provenance.** `rust_omoya-0.1.19` names a build window that *contains* the dmabuf commit.
- **The kernel-VT/grab question.** Measured absent; the consequences in both directions are unmeasured.
- **The hairetsu layout refusal** (`xkbcommon-hairetsu/src/xkb/mod.rs:219`) — reported by review, not re-confirmed here.
- **Per-capability handler cost.** The niri byte counts are measured; nothing else is. "A delegate plus a handler impl" is not a size estimate.
- The claim that the dmabuf mapping is uncached is an *inference* from throughput (8.3 MB/s read vs 2822 MB/s write on the same buffer); the PAT/MTRR attribute was not read. The ~80 ms residual is unattributed. The C benchmark reproduces nuri's loop *shape*, not its codegen.
- **Every colour claim here is dated 2026-08-20: re-measure, never infer.**

**Housekeeping owed from the measurement pass:** scratch files on plo at `/tmp/{cb.c,cb,kq.py,kq2.py,prof.bt,prof2.bt,mc.bt,mc2.bt,sock.bt,all.bt,bench.rs,nuri_vendored.rs,maps.txt,dis.txt,build.log}`.

---

## XI. Induct — the closing rite

- **Catalog entries land in the same commit** as their domain: `kukaku` in the tatara catalog; the bar in ayatsuri's/ishou's; `(defbackend omoya)` in `catalog/desktop.saihai.lisp`; every new kanshou leaf in `schema()` **with its round-trip test**.
- **Ratify through `/naming` before the crate exists:** `noki` (with the anchor objection on the record), `zaseki`, and the font/shaping leaf crate.
- **`contextualizify`** across `nix/CLAUDE.md` (plo is a desktop; the seat's gates; the GO/NO-GO), `omoya/docs/SEAT-PLAN.md` (the six corrections), `theory/` (a `SEAT.md` or QUADRO sibling naming the pixel drawer as the third face alongside terminal and web), `skill-map.d` (a `seat` skill covering the MCP leaves + `SeatLegality`), and memory (the flush-coupling finding; the `tatami`-is-taken corpus trap; the `pending-nuri-dmabuf-zerocopy` reversal; **`change_vt` is defined and never called**; **swash rasterizes to CPU bitmaps, so the fleet's text stack is CPU-reachable**).
- **`realizationify`** each phase — design → committed, adversarially-verified code with its done-predicate wired into CI.

---

## XII. What the substrate can generate or prove that it could not before

1. **egaku gains a pixel drawer, and the fleet gains a shared text stack.** Every shipped egaku widget becomes paintable into any framebuffer or `wl_shm` surface with no GPU; the fleet's UI algebra goes from one drawer to three faces — terminal, pixels, web. The *larger* win is the font/shaping leaf crate, which arrives with four consumers (garasu, egaku-nuri, asobi-text, escriba-render) instead of a guess at one.
2. **The fleet gains a proven space-partition algebra.** `kukaku` is a disjointness invariant over an explicit state, with a mockable seam and a real second consumer (mado, migrated, with a parity gate).
3. **A desktop's entire verb surface becomes generable from a catalog** — 284 rows with an executor and a matrix that fails the build on an unbacked arm and *publishes* its unwitnessed count.
4. **The seat becomes the first pleme-io surface where an agent and a human drive the same typed action vocabulary** — one body, two faces — under a legality gate that distinguishes an authored hold from an accidental one, marks every unwitnessed write as such, and reverts what it can revert from *outside* the process that might wedge.

**And the honest counterweight:** none of that is true today. Today the seat runs at 1.80 fps with a ~1.1–1.6 s keystroke round trip, has one window that can only be launched by argv, **no chord to open a second**, an invisible pointer, a VT escape that is defined and never called, no lock, no clipboard, no bar, no screenshot, no second monitor, no workspaces — and a `frames` counter that would keep rising through all of it.

---

### What changed after review

- **Two blocking facts the draft never mentioned are now the top of the document and the bulk of P1:** there is no keybinding path at all (`omoya -- <cmd>` is the only way to start a program), and `change_vt` is defined twice and called zero times while no `EVIOCGRAB` exists — so the documented rollback for this machine is not known to work and everything typed may also be reaching the tty. Cursor rendering, a greeter fallback session and a crash-supervision policy joined them. A GO/NO-GO box now gates plo becoming a sole seat.
- **Reordered.** P1 is now "the seat becomes usable" (deploy + latency edits + chords + cursor + VT + fallback + supervision); damage/vblank moved *ahead* of layout and merged with the write-only scanout into one `scanout.rs` rewrite; clipboard moved to P3 and lock to P4 — the latter on a corrected dependency: **`ext-session-lock` supplies its own surfaces, so lock never needed layer-shell.** Workspaces and foreign-toplevel management got their own phase (P9); the draft named them as convergent projects and scheduled neither, while 40 catalog rows already depend on them.
- **P0 descoped** from nine leaves to four, `Observe`-only. `Rehearsed` was circular — it needs capture, which is P10 — so it moved to P11, and every write until then is stamped `unwitnessed: true`. Dead-man revert is now classified by revert-direction (`lock`/`vt_switch` never auto-revert) and held by a separate supervisor, since a wedged compositor cannot revert itself.
- **Four predicates were vacuous and are replaced:** `painted/computed == 1.0` (satisfied by the null implementation → bound `computed_px` instead); "four non-zero latency legs" (two in-process legs plus injected-sleep red-runs; the cross-process pair has no correlation id and is a labelled estimate); `every_leaf_has_a_writer` (a writer *existed* for `owed_vt_switches` → round-trip, not presence); model-only disjointness (now paired with `pixel_at` and a `map_element` red-run). P11's matrix must publish its unwitnessed count.
- **Six ledger rows were rounded up and are re-graded** (layout mapping, clipboard retirement, decoration, notification filter, the credential seal re-attributed from a grep to `AuthProof`'s constructor, and the scanout seal re-scoped from a method-name trybuild to a private module plus a forbidden-symbol lint). Twelve rows were added for the newly-scheduled and newly-stated gaps.
- **P7 re-scoped and re-ranked.** "The only thing missing is a drawer" was wrong — `nuri` has zero font symbols. But "a second rasterizer for the fleet" is also wrong: `garasu/src/text.rs` already drives cosmic-text + `SwashCache`, and swash produces **CPU bitmaps**. The real deliverable is a shared font/shaping leaf crate extracted from garasu, with four consumers. The corpus-wide golden oracle was cut for a screenshot hash plus targeted glyph assertions.
- **P6's extraction was honest-scoped:** `geom` only, because snap zones and the drag FSM are floating machinery this plan chooses tiling to avoid — and mado's migration is now a named deliverable with a parity gate, not an implication. `apply` gained an explicit `LayoutState` (scrolling columns is stateful; the old signature could not express it and the proptest would have quantified over nothing). Popup grabs, fullscreen/maximize and a Firefox-hour acceptance test were added.
- **Evidence discipline:** the dmabuf revert now requires proving the `wl_shm` fallback first (mado is winit/wgpu with no buffer-protocol code; NVIDIA's WSI decides, and an llvmpipe demotion would read as a win); the baseline must be re-taken by store path, since `rust_omoya-0.1.19` names a build window containing the dmabuf commit; "deploy HEAD" is restated as HEAD-minus-`2af277a`; `default_opacity` moves into declarative HM config with its client-side limit stated. P1b's perf framing was replaced with its real payoff (~0.7 ms of latency, the invariant and free translucency) plus the 4K ceiling. Two null leverage claims ("38 delegates compiled in", "six are a delegate plus a handler") were deleted, with per-capability counts marked as owed.
- **Where the critiques were wrong or partial:** A's "second rasterizer" claim (above); A's XWayland population (correct that JetBrains and modern GTK/Qt are native, but Electron defaults to X11 — and B's addition of **Cursor** is the concrete fleet casualty, so the paragraph now names the editor); A's `noki` gloss objection is right and produced a deferral to `/naming` rather than a silent keep; B's "damage/vblank last" ordering was not taken — the usability items it was protecting all moved into P1, which leaves A's argument (the dependency on disjointness is soft, vblank needs nothing from layout) standing.
