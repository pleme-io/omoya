# shitsurai (設え) — the visual design of the pleme-io seat

**Status:** design, ready to implement. Every value below is a decision, not a range.
**Name:** `shitsurai` — the Japanese practice of *arranging a room*: choosing few things, placing them exactly, leaving the rest empty. Verified free across the fleet corpus (codesearch `search_repos`, 1152 repos, **0 hits**). It sits in omoya's existing house metaphor family — 母屋 omoya (main house) / 軒 noki (eaves, the bar) / 塗り nuri (the coating) / 区画 kukaku (the parcels).

---

## 1. The aesthetic, in one sentence

> **A quiet room, precisely arranged: flat opaque Nord planes separated by light and by space rather than by lines, one frost accent that only ever means *here*, and nothing that moves unless you moved it.**

Three corollaries that resolve every later argument:

1. **Depth is a luminance step, never a border and never a shadow.** Nord ships exactly four Polar Night rungs. That is the whole elevation budget; there is no nord0.5.
2. **Focus is presence/absence, never hue-A-vs-hue-B and never dimming.** The unfocused window has *nothing drawn on it*. That is simultaneously the strongest signal and the cheapest branch.
3. **Everything on screen is an opaque axis-aligned rectangle or a cached bitmap.** This is not a limitation we are working around — it is the same restraint the 2026 rice corpus converged on independently, arrived at from the other direction.

---

## 2. The palette — by role, never by band index

### 2.1 Source of truth

Colours are read as **roles**, resolved through `ishou_tokens::SemanticRoles::pleme_dark()` against `ishou_tokens::ColorPalette::pleme()`, which itself reads `irodori::NORD`. `omoya::theme` is the only module that may touch a colour; no other file names a band.

> **★ BLOCKER, fix before P1 lands the role names.** `ishou-tokens 0.1.10` pins `irodori = "^0.1"`; omoya pins `irodori = "0.2"`. Linking both today gives two `irodori::Color` types. `git diff aac7050..71014f3 -- src/` in irodori is **empty** — v0.1.1→v0.2.0 was a packaging-only bump. Fix is one line in `/Users/luis.d/code/github/pleme-io/ishou/Cargo.toml` (`irodori.workspace = "0.2"`) plus an ishou release.
>
> **Until that lands:** `omoya::theme` defines role-named accessors over `irodori::NORD` that mirror `SemanticRoles::pleme_dark()` *exactly*, plus a table test asserting each mapping. The swap afterwards is then mechanical, and the code never names a band index at a call site either way.

### 2.2 The elevation ladder — exactly four rungs, a fifth is forbidden

| role | token | hex | used for |
|---|---|---|---|
| `background` | `polar_night_0` (nord0) | `#2E3440` | the ground: desktop, gaps, everything behind everything |
| `surface` | `polar_night_1` (nord1) | `#3B4252` | noki (the bar); every future panel |
| `surface_elevated` | `polar_night_2` (nord2) | `#434C5E` | active/selected row inside a panel; hover-pressed |
| `text_dim` | `polar_night_3` (nord3) | `#4C566A` | **structural hairlines and disabled marks ONLY** |

Measured contrast inside this ladder: nord0→nord1 **1.241**, nord1→nord2 **1.166**, nord2→nord3 **1.169**, nord0→nord3 **1.693**. Every pair is below the 3:1 non-text floor, which is the whole point: these read as *depth*, not as edges. It is also the reason a fill change alone can never terminate a surface — see §4.2.

### 2.3 Text

| role | token | hex | CR on nord0 / nord1 | rule |
|---|---|---|---|---|
| `text_muted` | `snow_storm_0` (nord4) | `#D8DEE9` | **9.25 / 7.45** | **body text.** Everything readable defaults here. |
| `text` | `snow_storm_2` (nord6) | `#ECEFF4` | 10.84 / 8.73 | **emphasis only** — the one item in a group that is focused. Nord's own docs: "text that must be noticed." |
| `text_dim` | `polar_night_3` (nord3) | `#4C566A` | **1.69 / 1.36** | **NEVER text that must be read.** Empty-slot marks, hairlines, disabled glyphs. |

The role names read backwards on purpose and this is the single most-broken thing about Nord in the wild. `text` (nord6) at 10.84:1 is harsh for all-day reading; `text_muted` (nord4) at 9.25:1 is the body face. Hierarchy comes from `text_muted → text`, never from `text → text_dim`. **`text_dim` at 1.69:1 is legible in a screenshot at full brightness and gone at 40% in daylight.**

### 2.4 The accent — one, and it is `primary`

| role | token | hex | CR on nord0 / nord1 |
|---|---|---|---|
| `primary` | `frost_1` (**nord8**) | `#88C0D0` | 6.24 / 5.03 |

`primary` means **"here"** and nothing else: the focus ring, the active parcel cell's underline. It appears at most twice on screen.

**Trap, stated because it will be hit:** in `SemanticRoles::pleme_dark()`, the role literally named `accent` is `aurora_purple`, **not** frost. The frost accent is `primary`. Reaching for `accent` because it sounds right gives you purple.

The code today uses `NORD.frost[2]` = **nord9 `#81A1C1`** in two places (`bar.rs:160`, `theme.rs:210`). Both move to `primary` (frost[1] / nord8). nord9 is Nord's *secondary* frost and is 1.35:1 against nord8 — the fleet has been rendering its own accent as the wrong one.

### 2.5 State — Aurora, and only when the system speaks

A correct seat shows **zero Aurora pixels**. When it must:

| condition | render |
|---|---|
| warning | `warning` (`aurora_yellow`, nord13 `#EBCB8B`) as **foreground** — 8.00:1 on nord0, 6.44:1 on nord1 |
| error | `text` (nord6) **on a filled** `error` (`aurora_red`, nord11 `#BF616A`) pill |
| success | `success` (`aurora_green`, nord14 `#A3BE8C`) as foreground — 6.13 / 4.94 |

**`error` (nord11) is never a foreground colour.** Measured: 3.05:1 on nord0, **2.46:1 on nord1** — it is the *least readable colour in the palette*, and the naive `error = nord11` mapping produces the worst-legible glyph in the theme at exactly the moment the operator most needs to read it. `error` is a fill; nord6 is the text on it. Same for `frost_3` (nord10, 3.10 / 2.50) — recessive chrome only, never a label.

This must be typed, not documented: `theme` exposes state colours as **`(fg, bg)` pairs**, so "nord11 as a foreground" has no representation.

### 2.6 The scale — one 4px grid, from `ishou_tokens`

Do **not** mint a new scale. Use `ishou_tokens::Spacing` (`px_1`=4 … `px_32`=128) and `ishou_tokens::Radius` (`sm`=4, `md`=8, `lg`=12).

Permitted distances in shitsurai: **4, 8, 12, 16, 20, 28**. A 7 or a 13 anywhere in a layout expression is a defect. `bar.rs`'s current `let pad = 10_usize` is off-grid and is one of the reasons the strip reads as "assembled" rather than "typeset."

---

## 3. Geometry — exact values

### 3.1 noki (the bar)

| property | value | token / derivation |
|---|---|---|
| height | **28 px** | `Spacing::px_7` (4×7). Dead centre of the measured 26–30 band (Waybar 30, omarchy 26, yambar 26). Unchanged. |
| background | `surface` (nord1) | opaque, alpha **1.0**, always |
| body text colour | `text_muted` (nord4) | was nord5 |
| emphasis text colour | `text` (nord6) | focused cell only |
| empty/disabled colour | `text_dim` (nord3) | never a readable label |
| font | **JetBrainsMono Nerd Font, Regular, 13.0 px** | `FleetDefaults::prescribed().font_family` / `.font_size` |
| text/bar ratio | 13/28 = **0.46** | inside the measured 0.43–0.50 band |
| horizontal inset | **12 px** | `Spacing::px_3` (was 10, off-grid) |
| intra-group gutter | **8 px** | `Spacing::px_2` |
| inter-group gutter | **16 px** | `Spacing::px_4` — *this* is the separator; no glyph separators anywhere |
| cell (pill) height | **20 px** | 4 px inset top and bottom → 20/28 = 0.71, the measured 0.65–0.70 band |
| cell radius | **4 px** | `Radius::sm`. Not `full` — a lozenge reads Material; 4 reads precise. |
| bar corner radius | **0** | edge-anchored and flush. A rounded bar against the screen edge leaves two slivers of ground at the top corners that read as a rendering bug. |
| bottom terminator | **1 px `text_dim` (nord3)**, full width | CR 1.36 against nord1 — the load-bearing line, see §4.2 |
| active-cell mark | **2 px `primary` (nord8)**, bottom 2 rows, spanning the active cell only, overpainting the hairline | |
| translucency | **none** | see §5.2 |

**Baseline.** Optically centre on **cap height**, not the line box: `baseline = round(h/2 + cap_height/2)`. Derive `cap_height` from `font.horizontal_line_metrics(13.0)` / the `'H'` glyph metrics — **not** the current inlined `FONT_PX * 0.35`. The magic constant happens to be right for DejaVu and JetBrainsMono (both ≈0.70–0.73 em) and will silently shift the baseline a pixel the moment the face changes.

**Digits are tabular.** JetBrainsMono is monospace so this is free — and here it is a *performance* rule, not a typographic one: with variable advances, a minute rollover changes the width of the clock string, which moves the origin of everything right of it, which makes the honest damage rect the whole side of the bar.

**Advance accumulation is f32, rounded only at the pixel write.** `bar.rs:192` and `:235` both do `advance_width as usize` — an f32→usize *truncation* per character. JetBrainsMono's 0.6 em advance at 13 px is 7.8 px, losing 0.8 px per char; over a 50-char line that is **40 px** of cumulative error, and because `right_x = w - (right_w + pad)` is computed from the same truncated sum, the right side is both mis-measured and mis-drawn.

### 3.2 Windows and gaps

| property | value | change |
|---|---|---|
| `layout::GAP` | **4** | was 8 |
| gap between two adjacent windows | **8** (4+4) | was 16 — the loudest maximalist tell at 1080p |
| gap at every screen edge | **4** | including the top, so 4 px of `background` shows under noki's hairline and the bar reads as a plane *above* the ground without any translucency |
| `layout::BORDER` | **2** | unchanged; still drawn inside the gap so focusing never resizes content |
| residual ground between two adjacent windows' borders | 8 − 2 − 2 = **4 px** | invariant `GAP >= BORDER*2` holds at exactly the floor; `right_left_edge - left_right_edge >= BORDER*2` → 8 ≥ 4 ✓ |
| window corner radius | **0** | see §5.5 |

### 3.3 Focus

- **Focused window:** a **2 px ring** in `primary` (nord8), drawn as four `fill` rects in the gap.
- **Unfocused window:** **nothing is drawn.** No ring, no dim, no desaturation, no tint. Absence *is* the encoding.

Cost, measured on the real crate: four 2 px edges = **0.006 ms**, 0.2% of the 2.78 ms budget. This is the cheapest possible focus indicator and it is stronger than any of the alternatives in §5.

### 3.4 The pointer

Unchanged in shape and size (10×17 ASCII mask at `SCALE = 2` → 20×34), which blits at **0.001 ms**. Two changes only: source its two colours by role — fill = `text` (nord6, "must be noticed"), outline = `background` (nord0) — and keep the mask **binary**. Antialiasing it is free (the raster is cached) but a hard-edged pointer is more legible over arbitrary client content, which is the job.

---

## 4. noki's content — three zones

Today the bar says `" wayland-1"` on the left and `"N windows   HH:MM UTC"` on the right. All three are deleted. The socket name is developer telemetry that already lives in `introspect`; the window count tells you nothing you cannot see by looking; `UTC` is a correctness admission, not information.

| zone | content | colours |
|---|---|---|
| **left** | **parcel cells.** One 20×20 cell per kukaku leaf on the current output, max 9, laid out on fixed 20 px slots with an 8 px gutter. Each cell carries its 1-based index as a digit. | focused: digit in `text` (nord6) + 2 px `primary` underline. others: digit in `text_muted` (nord4). unused slots: not drawn. |
| **centre** | **`HH:MM`, local time**, screen-centred at `round(w/2 - measure/2)` **snapped to an even pixel** | `text_muted` (nord4) |
| **right** | **empty**, until the system has something to say | §2.5 |

Three points that are load-bearing rather than stylistic:

- **Centre means centred on the SCREEN**, not centred in the space between the left and right groups. A flex-centred clock drifts a few pixels every time a parcel appears; a screen-centred one is the only element on the desktop with a permanently constant rect, which makes it the cheapest thing to redraw.
- **Snap the centre origin to an even pixel** or the same string rasterizes to two different bitmaps at two sub-pixel offsets and defeats the "only re-rasterize when the text changes" rule.
- **Local time, honestly.** Resolve via `TZ` / `/etc/localtime`. If the seat cannot resolve a zone, render `HH:MM UTC` in `warning` (nord13) — an honest degraded state, never a silent lie.
- **The parcel cells become workspaces later with no visual change.** omoya has no workspaces; kukaku already knows the leaves. Building the cell row against leaves now means workspaces are a data swap, not a redesign.

**No icon glyphs in v1.** Every indicator is a `fill` rect. Reason: `FleetDefaults` deliberately selects the **non-Mono** `JetBrainsMono Nerd Font` (correct for mado's cell grid, where wide icons keep their designed width). In a naive pen-advance rasterizer like `bar.rs`, a double-width icon glyph overlaps its neighbour. If icons are wanted later, add `nerd-fonts.symbols-only` (`SymbolsNerdFontMono-Regular.ttf`) as a **second** `fontdue::Font` at its own size and its own baseline offset — never a patched text face.

> **Prerequisite, verify on plo before P1.** `bar::rasterize` returns `None` when no font is found in any fontconfig-declared root, and `drm.rs` then pushes no bar element — silently, by design. The WANTED list leads with JetBrainsMono and falls back to DejaVu. Confirm `fonts.packages` on plo actually carries the face; the vkms gate installs only `dejavu_fonts`.

---

## 5. What we are NOT doing, and why

Each of these is beautiful somewhere. None of them is affordable or correct here. Priced against the measured **2.023 ns/px** (`frame_us = 4194` over 1920×1080) and the **2.78 ms** budget at 360 Hz, using the benchmarked nuri numbers.

### 5.1 Blur — of any kind, live
Dual-Kawase 3-pass over one 960×1080 window is 6,220,800 px-ops = **12.58 ms = 453% of the frame budget**. Full-screen is **906%**. A separable 15-tap Gaussian is ~30 blends/px, and a *single* blend over the full screen already measures **2.004 ms**. There is no cheaper kernel that changes the order of magnitude and no approximation that reads as blur. **The cheap thing that reads as *good* is an opaque flat panel with a hairline** — which is exactly what §3.1 specifies, and which yoru (the most restrained rice in the corpus) configures dual_kawase and then deliberately doesn't use.

### 5.2 Translucency on noki
`nuri::Surface::blit`'s fast path requires `alpha >= 1.0`. Below it, the entire 1920×28 strip drops off `copy_from_slice` into the per-pixel path. And it buys nothing: over a flat `background` ground, a 94%-opaque `surface` bar is **pixel-identical** to an opaque blend. Compute the composite ONCE with `ishou_tokens::blend_linear(bg, tint, alpha)` into an opaque `Rgb` and `fill` it — identical look, memcpy speed, and the bar's damage stops depending on what is underneath it. This is exactly how Vellum's GLASS band is already born in the fleet.

### 5.3 Dimming / opacity on unfocused windows
One full-screen alpha pass measures **2.004 ms** — 72% of the budget, 29× the opaque fill. `alpha < 1.0` is precisely the config that reintroduces the per-pixel loop the compositor was fixed to escape (700 ms → 4 ms frames). It buys a 5–10% luminance difference the eye must compare across the screen; the 2 px `primary` ring answers the same question with a hard high-contrast signal for **0.006 ms**. Hard no.

### 5.4 Drop shadows
Composited over `background` (#2E3440), niri's shipped default (`softness 30, spread 5, offset y=5, #0007`) yields **#20252E — a contrast ratio of 1.23:1**, i.e. *less* separation than the free nord0→nord1 elevation step. The ring costs **254.8 µs (9.2% of budget)** redrawn on every move frame. The inverse cue works and is what macOS dark and libadwaita dark both switched to: **a 1 px `text_dim` (nord3) hairline along a surface's top edge**, CR 1.36, costing **1.9 µs**. Use the hairline. Do not build shadows in nuri.

### 5.5 Rounded window corners
Rejected on **aesthetics**, not cost. omoya is a tiler; libadwaita's own rule is radius → 0 when a window is tiled or maximized, because rounded corners on a tiled window cut holes that read as a defect in the tiling rather than as softening. Radius is reserved for genuinely **floating** surfaces (future launcher, notification) at `Radius::md` = 8, and for noki's 20 px cells at `Radius::sm` = 4.

### 5.6 Window open / close / move / resize animation
At 360 Hz a 200 ms animation is **72 forced composites**. A full-window fade is 72 × ~2 ms. kukaku placement is deterministic — place instantly. Animate chrome, never content.

### 5.7 Workspace slide transitions
2.07 M px every frame for the whole transition. It converts a zero-frame idle desktop into a 100%-of-a-core burst, which is a regression of the seat's best property. The *information* a slide carries (which direction you went) is carried just as well by the parcel cell's underline moving — 20×2 px.

### 5.8 Live window thumbnails / a zoomed overview
Requires compositing every window every frame *plus* a downscale per window. nuri samples **nearest-neighbour** (`pending-nuri-filtering`), so the downscale is both slow and visibly aliased. If an overview is ever wanted, the affordable form is a **typographic** one — a centred, well-set list of parcel indices and window titles over the ground — which plays to the strength this seat actually has.

### 5.9 A per-second clock
`drm.rs`'s Chrome timer deliberately marks `Owed` only on a **minute** boundary. Going per-second re-introduces 1 fps on an idle seat. The clock shows `HH:MM`. Seconds are not information a desktop owes you.

### 5.10 A wallpaper
Contrarian, and stated with the number. A solid `background` fill measures **0.070 ms** (LLVM vectorizes nuri's fill loop into a memset); a full-screen wallpaper blit measures **0.626 ms** — **9× more expensive for the same pixels**, and only free at exactly 1:1 (off `one_to_one`, nuri does a transform map-back plus two divisions per pixel with nearest-neighbour sampling). With 8 px gaps the wallpaper is visible only as thin lines between windows and a 4 px band under noki, where a photo reads as noise along every window edge. **The ground is a flat `background` fill.** An empty, arranged room is the point.

### 5.11 Noise / dithering
Only needed where a gradient bands. shitsurai has no gradients — every surface is a flat opaque fill. Keep the 64×64 blue-noise tile in pocket for the day a gradient is genuinely wanted; do not build it now.

### 5.12 What motion IS permitted — exactly one thing
**The focus-ring colour cross-fade.** `ishou_tokens::Motion::default().duration.fast_ms` (**150 ms**) on `easing.decelerate` — **monotonic, no overshoot**. Overshoot simulates mass; a window has mass, a colour does not, and bouncing a colour is the single loudest "busy" tell in the corpus. Enforce it as a type: expose `motion.spatial.*` (overshoot permitted) and `motion.effect.*` (monotonic only) and give no widget a path from a colour to a spatial curve.

Cost: 4 × 2 px edges = ~10,000 px per frame. Gated by two policies:
- **Cadence:** present animation frames at most every **3rd vblank** (120 Hz). 150 ms is then **18 frames**, not 54. 120 Hz motion with correct easing looks better than 360 Hz motion with inconsistent pacing.
- **Quantisation:** suppress the present when the interpolated colour rounds to the same 8-bit value it had last frame. A decel tail at 360 Hz produces many literally-identical frames.
- **Respect `FleetDefaults::prescribed().reduce_motion`.** When true, snap.

This is P5 and optional. Ship instant first; the seat is complete without it.

---

## 6. New primitives required

| # | primitive | where | why |
|---|---|---|---|
| **1** | `Surface::blit_mask(&mut self, dst: Rect, mask: &[u8], mask_stride: usize, color: Rgba)` — 8-bit coverage × solid colour, source-over | `crates/nuri/src/lib.rs` | The one genuinely missing op. Retires `bar.rs`'s hand-rolled per-glyph blend, and serves cell corner masks and any future floating surface's corners from the same code. Pure integer, zero-dependency, no `unsafe` — nuri's stated invariants survive. |
| **2** | `blit(..., opaque: OpaqueHint)` — the caller states pixel opacity instead of nuri rediscovering it | `crates/nuri/src/lib.rs` + `crates/omoya/src/nuri_renderer.rs` | The fast path does `srow.chunks_exact(4).all(\|px\| px[3] == 0xff)` per row — a full second read pass costing **0.519 ms of a 0.626 ms** full-screen blit (**83%**, 5× the memcpy it guards). `nuri_renderer::normalise_opaque` already forces `alpha = 0xff` for every `Xrgb8888` texture at import and throws the fact away. Opacity is a property of *pixels*, not of a format, so this does not breach nuri's format-free doctrine. **Largest measured saving available anywhere in this document.** |
| **3** | Space-typed theme + one `to_nuri` adapter | `crates/omoya/src/theme.rs` | Replace hand-rolled `srgb_to_linear` with `ishou_tokens::space::{Srgb, Linear}`; return space-typed values instead of bare `[f32;4]`; add `fn to_nuri(c: Srgb, alpha: f32, fb: FramebufferSpace) -> nuri::Rgba` owning premultiplication **and** the B,G,R,A memory order in ONE place. Three consumers re-derive channel order today. This upgrades the §7-P0 bug class from *only-mitigated* to *parse-time-rejected*. |
| **4** | Glyph atlas + gamma LUT | `crates/omoya/src/bar.rs` | `HashMap<(char, u32), (Metrics, Vec<u8>)>` behind the existing `OnceLock`; hoist `fontdue::Font::from_bytes` (currently re-parses the whole TTF on every `rasterize()`); one `[u8; 256]` LUT per `(fg, bg)` role pair. 95 printable ASCII at 13 px ≈ 12 KB of atlas; a handful of KB of LUTs. |
| **5** | Per-cell bar damage | `crates/omoya/src/bar.rs` + `drm.rs` | `struct Cell { rect: Rect, content: String, deco: Decoration }`, layout computed once per mode, redraw only cells whose content changed. |
| **6** | `Owed::Motion` (**only if P5 ships**) | `crates/omoya/src/owed.rs` | An eighth mekuri cause; `the_catalog_is_complete`'s count goes 7 → 8. Flagged deliberately: this file is the forcing function that makes you price an animation before you write it. |

**Explicitly NOT needed:** rounded-rect SDF, gradient fill, bilinear sampling, blur, shadow, path rendering, compositing operators beyond source-over. Do not add them.

### Decoration vocabulary — the closed set

Model a cell's decoration as a closed enum over what nuri can actually draw. yambar arrived at the same five independently, and every one is an axis-aligned rect:

```rust
enum Decoration { None, Background(Role), Underline { px: u8, role: Role }, Overline { px: u8, role: Role }, Border { px: u8, role: Role } }
```

That is the honest version of a design token set: it cannot express anything the rasterizer cannot draw.

---

## 7. Work order — highest (visual impact / cost) first

### P0 — `focus_border_for_surface` is inverted, and the accent is the wrong frost
**Confirmed by reading the file.** `theme.rs:209-226` does `if format_is_srgb { srgb } else { linear }`. Its two siblings do the opposite: `background_for_surface` (`:85-91`) is `if format_is_srgb { background_linear() } else { background_srgb() }`. `drm.rs:981` passes `false` (correct for the plain `DRM_FORMAT_ARGB8888` dumb buffer), so the border takes the **linear** branch and writes linear bytes into a framebuffer that converts nothing. `#81A1C1` = (129,161,193) is currently painting as **rgb(56,91,136)** — a muddy dark navy where the accent should be. The module's own header, doc comment and regression test all document this exact class; the background has a test pinning it and the border has none.

- **Change:** swap the branch; change `NORD.frost[2]` → the `primary` role (frost[1] / nord8); add `a_non_srgb_surface_gets_the_raw_nord_bytes` for the border mirroring the background's.
- **File:** `crates/omoya/src/theme.rs`
- **Difficulty:** trivial (~10 lines + a test)
- **Per-frame cost:** unchanged (0.006 ms for four edges)
- **Impact:** the only per-window chrome on the seat goes from invisible-and-wrong-hue to the palette accent. Highest visual return per line in this document, and it is a *bug fix*, not a design change.

### P1 — the opaque hint on `nuri::blit`
Zero visual impact; ordered here because everything below spends the budget it returns.

- **Change:** primitive #2. Thread `OpaqueHint` from the import path (`normalise_opaque` already knows) through `render_texture_from_to`'s `fast` predicate into `blit`. Keep the two preconditions written next to each other as the source comment demands.
- **Files:** `crates/nuri/src/lib.rs`, `crates/omoya/src/nuri_renderer.rs`
- **Difficulty:** medium (an API change with two call sites and a benchmark)
- **Per-frame cost:** **returns ~0.5 ms**, ~18% of the frame budget, on any frame with a full-screen client
- **Verify:** `introspect`'s `blit_fast` / `blit_slow` counters, and re-measure `frame_us` on plo. Do not trust the derived 2.023 ns/px afterwards — re-derive it.

### P2 — the typographic pass on noki
Type *is* the design in a 28 px strip. This is the largest single visual change available.

- **Change:** face → `FleetDefaults::prescribed().font_family` at 13.0 px; body colour nord5 → `text_muted` (nord4); accent line nord9 → `primary` (nord8) and thickness 1 → the §3.1 split (1 px `text_dim` full-width hairline + 2 px `primary` under the active cell only); f32 pen with rounding at the write; baseline derived from `horizontal_line_metrics`; `pad` 10 → 12; **gamma-correct blending via a 256-entry LUT per role pair**; glyph atlas; `Font::from_bytes` hoisted into the `OnceLock`; `font.metrics` called once per char instead of twice.
- **File:** `crates/omoya/src/bar.rs` (~150 lines, one file)
- **Difficulty:** medium
- **Per-frame cost:** **unchanged** (strip blit stays 0.017 ms). Rasterize cost drops, and rasterize runs once a minute anyway.
- **Why the LUT matters:** `bar.rs:216-220` blends glyph coverage with `dst*(1-a) + src*a` on **raw sRGB bytes**. Coverage is a linear quantity; sRGB bytes are not. Measured for the actual colours: a requested coverage of 0.25 is *seen* as 0.125 (**50% loss**), 0.5 is seen as 0.327 (**35% loss**). On a dark ground the naive result is always too dark, so antialiased stems render thin and slightly starved — the effect people misdiagnose as "the font is too thin" and wrongly fix with a heavier weight. Because fg and bg are both compile-time role constants, the entire blend collapses to three array indexes. **It is correct *and* strictly faster than what is there now.**

### P3 — gaps, focus ring, and the unfocused rule
- **Change:** `layout::GAP` 8 → 4. `BORDER` stays 2. Ring drawn on the focused window only; **nothing** drawn on unfocused windows (this is already the behaviour — make it a stated invariant with a test, so a future "let's dim the others" has to argue with a test rather than with a comment).
- **File:** `crates/omoya/src/layout.rs` (two constants), `crates/omoya/src/drm.rs` (the element push)
- **Difficulty:** trivial
- **Per-frame cost:** 0.006 ms, unchanged. Gaps are ground showing through, which is a `fill`, which is a memset — geometry here is free.

### P4 — `blit_mask`, cell decorations, per-cell damage
- **Change:** primitive #1 in nuri; the `Cell` / `Decoration` model in `bar.rs`; a fixed layout computed once per mode; redraw and damage only changed cells. Cell corner masks at r=4 precomputed once as a single 4×4 quadrant (16 bytes), indexed mirrored.
- **Files:** `crates/nuri/src/lib.rs`, `crates/omoya/src/bar.rs`, `crates/omoya/src/drm.rs`
- **Difficulty:** medium-high (the damage plumbing through `MemoryRenderBuffer::render().draw(...)` is the real work)
- **Per-frame cost:** a minute rollover goes from **53,760 px (0.017 ms)** to **~1,000 px (~0.002 ms)** — a 50× reduction on the one thing that fires on an otherwise idle seat.
- **★ Element-identity trap:** every new element needs a **stable smithay `Id` held across frames** (see the existing `border_ids: [Id; 4]`), or the damage tracker reads it as "old one vanished, new one appeared" and re-damages both rects every frame — silently turning partial repaint back into full repaint.

### P5 — the focus-ring cross-fade (optional)
- **Change:** `Owed::Motion` + catalog count 7 → 8; a `Motion { Spring, Easing }` value driven by *real elapsed time* (not a frame count, or the reduced cadence changes the animation's speed); the 120 Hz cadence gate; the whole-8-bit-step suppression gate; `reduce_motion` respected.
- **Files:** `crates/omoya/src/owed.rs`, `crates/omoya/src/drm.rs`, a new `motion.rs`
- **Difficulty:** medium
- **Per-frame cost:** 10,000 px × 18 presented frames ≈ 0.02 ms/frame, 0.7% of budget, once per focus change
- **Gate before merging:** `frames` and `presented` in `introspect` must remain equal-and-zero on an idle seat. If they diverge, it is a regression regardless of how it looks.

### P6 — hygiene, zero visual cost, high cognitive return
The `SeatElements` variant names in `drm.rs:~387` are **inverted against their use**: the mouse cursor is pushed as `SeatElements::Bar(...)` (it is a `MemoryRenderBuffer`) and the four focus-border edges are pushed as `SeatElements::Cursor(...)` (they are `SolidColorRenderElement`s). Rename to `Texture` / `Solid` / `Space` — named for what they *are* — before anyone adds a fourth. Aesthetic and cognitive care is load-bearing.

---

## 8. Config surface

The values above land as a typed `OmoyaVisualConfig` implementing **both** `shikumi::TieredConfig` and `ishou_tokens::FleetThemedConfig`:

```rust
impl ishou_tokens::FleetThemedConfig for OmoyaVisualConfig {
    fn from_fleet(fd: &ishou_tokens::FleetDefaults) -> Self {
        Self {
            theme: fd.theme,
            font_family: fd.font_family.clone(),
            font_size: fd.font_size,           // 13.0 — NOT a literal here
            reduce_motion: fd.reduce_motion,
            bar_height: 28, gap: 4, border: 2, // Spacing-derived
            ..<Self as shikumi::TieredConfig>::bare()
        }
    }
}
```

plus exactly one drift test:

```rust
#[test]
fn omoya_converges_with_fleet() {
    let d = <OmoyaVisualConfig as shikumi::TieredConfig>::prescribed_default();
    ishou_tokens::convergence::Guard::for_app("omoya")
        .expect_theme(d.theme)
        .expect_font_family(&d.font_family)
        .expect_font_size(d.font_size)
        .run();
}
```

Read `mado/src/config.rs:4895-4935` first — it documents why an `assert_ne!` there once **enshrined** a split instead of catching it. Assert equality.

**Do not hand-copy `FleetDefaults` values into `prescribed_default()`.** That is the documented anti-pattern and it is how a fleet change silently stops propagating.

---

## 9. The admission test for any future effect

Encode it rather than arguing it case by case:

> **An effect whose damage region is O(perimeter) is admissible. An effect whose damage is O(area) of a window or the screen is not.**

Every item on shitsurai's yes-list is chrome geometry — edges, corners, small fills, cached bitmaps. Every item on the no-list is whole-surface treatment — opacity, dim, blur, scale. That split maps almost exactly onto the professional-vs-busy split the rice corpus arrived at independently, which is a happy accident worth leaning on.

Concretely: each render element declares its worst-case pixel-touch count and a test sums the plausible-simultaneous set against **1,373,391 px** (the 2.78 ms budget at 2.023 ns/px). It fails loudly the first time someone adds a full-window pass. `docs/DESKTOP-PLAN.md` P8 already names `damage.computed_px < 20_000` for an animated element; nothing measures it. Make `Pass` carry the computed damage area and this stops being prose.

**And re-measure.** `SMOOTHNESS.md`'s closing rule was earned three times over: three of its four changes were justified by a plausible story about where time goes, and three of those stories were wrong. Every number in this document that came from a derivation rather than a benchmark should be re-taken on plo after P1 lands, because P1 changes the denominator.

---

## 10. First three commits

**1. `omoya: the focus ring is nord8, and it is the colour it says it is`**
`crates/omoya/src/theme.rs` — swap the inverted branch in `focus_border_for_surface`; move the accent from `NORD.frost[2]` (nord9) to the `primary` role (`frost[1]`, nord8); add the missing non-sRGB regression test mirroring `a_non_srgb_surface_gets_the_raw_nord_bytes`. Ten lines, and the seat's only window chrome stops rendering as `rgb(56,91,136)`.

**2. `nuri: the caller states opacity; stop rediscovering it per row`**
`crates/nuri/src/lib.rs` + `crates/omoya/src/nuri_renderer.rs` — add `OpaqueHint` to `blit`, thread it from `normalise_opaque`, keep the two preconditions adjacent. Add a `blit_scan_us` row to `introspect` and record before/after `frame_us` on plo in the commit message. Returns ~0.5 ms/frame — the budget every later commit spends.

**3. `omoya/noki: 13px on the fleet face, gamma-correct, tabular, cached`**
`crates/omoya/src/bar.rs` — the P2 typographic pass in one file: fleet face at 13 px, `text_muted` body / `text` emphasis / `primary` accent by role, f32 pen, metrics-derived baseline, 12 px inset, the 256-entry gamma LUT, the glyph atlas, the hoisted font parse. Delete `" wayland-1"` and `"N windows"` in the same commit — a deletion is the honest done-predicate.

---

## Appendix — the contrast table this design is built on

Foreground on `background` (nord0) / on `surface` (nord1). WCAG AA normal text needs 4.5; the non-text UI floor is 3.0.

| token | hex | on nord0 | on nord1 | verdict in shitsurai |
|---|---|---|---|---|
| nord6 `text` | `#ECEFF4` | 10.84 | 8.73 | emphasis only |
| nord5 | `#E5E9F0` | 10.26 | 8.26 | unused |
| nord4 `text_muted` | `#D8DEE9` | **9.25** | **7.45** | **body text** |
| nord13 `warning` | `#EBCB8B` | 8.00 | 6.44 | warning fg; the one bright accent |
| **nord8 `primary`** | `#88C0D0` | **6.24** | **5.03** | **the accent — focus, and nothing else** |
| nord14 `success` | `#A3BE8C` | 6.13 | 4.94 | success fg (only Aurora clearing AA on nord1) |
| nord7 | `#8FBCBB` | 5.99 | 4.83 | unused |
| nord9 | `#81A1C1` | 4.64 | 3.74 | **retired from both current uses** |
| nord15 | `#B48EAD` | 4.41 | 3.55 | unused |
| nord12 | `#D08770` | 4.39 | 3.54 | unused |
| nord10 | `#5E81AC` | 3.10 | **2.50** | recessive chrome only — never a label |
| nord11 `error` | `#BF616A` | 3.05 | **2.46** | **fill only, never a foreground** |
| nord3 `text_dim` | `#4C566A` | **1.69** | **1.36** | hairlines and empty marks only |