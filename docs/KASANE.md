# kasane (重ね) — the GPU pipe, and the router that decides which pipe

> **Status: DESIGN. No code exists.** Measured 2026-09-03. Every capability
> claim below was probed on plo's real hardware or read from omoya's source;
> nothing here is inferred. Re-measure before acting on any number.

---

## 0. The problem, measured

Every GPU client on plo renders on the **CPU**. From plo's journal:

```
WARN garasu::ctx: no hardware GPU adapter on this machine — rendering on the CPU
     adapter="llvmpipe (LLVM 21.1.7, 256 bits)" ... backend=Vulkan device_type=Cpu
```

That message was false and is fixed (`garasu@e3dad74`) — plo holds a GeForce RTX
3070 with driver 580.142 loaded. The adapter is **refused**, not absent, and the
refusal comes from us:

1. omoya composites on the CPU (`nuri`), deliberately — no GPU driver
   dependency, no `unsafe`, no dmabuf import path.
2. It therefore advertises **linear-modifier dmabuf only**. A tiled modifier
   describes a layout only a GPU can decode, and a CPU blitter reading one
   paints structured noise. **Linear-only is correct for nuri.**
3. NVIDIA does not present linear.
4. So no hardware adapter can present to an omoya surface, and every GPU client
   — `mado` included, a GPU terminal — falls back to `llvmpipe`.

This is the likeliest single explanation for "the look and feel is absolutely
just bad" and the slow, wrong-looking startup. It is architectural, not
cosmetic: polishing rounding and bar modules on top of it is polishing a
software-rendered desktop.

---

## 1. The destination

**omoya composites on the GPU when there is one, on the CPU when there is not,
and routes individual work to whichever pipe suits it — with the choice as a
typed value, not a branch.**

Stated as the operator did: *"code omoya to be a GPU and CPU compositor and be
able to use both fluidly and completely… send calculations that are GPU through
one pipe and send calculations that are good for CPU to another."*

---

## 2. What is ALREADY ready — the reason this is tractable

Three seams exist. None of them needs to be built or restructured.

| seam | state | where |
|---|---|---|
| The render loop is **generic over the renderer** | shipped | `drm.rs`: `R: Renderer + ImportMem + Bind<Dmabuf> + ArmFlush + ExportMem`, `R::TextureId: Clone + Send + Texture` |
| dmabuf advertisement is **derived from the renderer** | shipped | `drm.rs`: `renderer.dmabuf_formats()` passed straight through — *"never a hand-written list"* |
| The **typed route vocabulary** | shipped, half-wired | `rouka.rs`: `Route::{DirectScanout, GpuComposited, CpuReadback}`, `Cost { cpu_bytes_per_frame }`, `is_zero_copy()`, `choose` |

**A second renderer is expressible today.** And because the dmabuf global is
derived rather than hand-listed, the moment a GPU renderer is selected the
compositor advertises tiled modifiers and NVIDIA can present — *that single
change is the whole fix for the llvmpipe problem*, with no protocol work.

`rouka`'s `GpuComposited` and `DirectScanout` variants are "never constructed"
today. They were designed for this and are waiting for a second pipe to exist.

---

## 3. The capability probe — the research risk, retired

The design rests on the driver offering dmabuf interop. Measured on plo, on the
actual card:

```
GPU0: NVIDIA GeForce RTX 3070
    VK_EXT_external_memory_dma_buf   = True
    VK_EXT_image_drm_format_modifier = True
    VK_KHR_external_memory_fd        = True
GPU1: llvmpipe (LLVM 21.1.7, 256 bits)
    (same three — so the CPU adapter can exercise the same path in CI)
```

All three are present. **llvmpipe supporting them too is a gift**: the import
path is testable on a machine with no GPU, which is most of CI.

---

## 4. Why Vulkan and not EGL/GLES — containment, not preference

Operator instruction: *"we still aren't linking C libraries" / "we must write in
Rust" / "we go down to the lowest layer and surround the C with Safe Rust and
keep it contained."*

The intuitive choice is smithay's `GlesRenderer` (EGL + GBM), which every Wayland
compositor uses. It is rejected, and the reason **inverts the intuition**:

> EGL/GLES is a C abstraction stacked *on top of* the driver. It is **more** C
> surface, not less, and the unsafe spreads across a large API.

Raw Vulkan via `ash` is the **thinnest C boundary available**: one ABI, pure-Rust
pre-generated bindings, `dlopen("libvulkan.so.1")` at runtime — no `-sys` crate
compiling C, no `bindgen` on system headers, no build-time linkage. Everything
above the seam is ours.

The irreducible remainder is stated rather than hidden: the vendor driver behind
the loader is C, and no Rust can reach a GPU without it. That is a fact about
the world, so it is **typed** (loader absent ⇒ typed fallback to nuri), not
pretended away.

**Precedent already shipped in this repo:** omoya speaks logind over **D-Bus in
pure Rust (`zbus`)** rather than linking `libseat`, and reads input through the
pure-Rust **`evdev`** crate rather than `libevdev`. `backend_session_libseat` and
`renderer_pixman` were both REMOVED for exactly this reason. Speak the protocol
in Rust; do not link the library.

---

## 5. The architecture

```
   client surface (tiled dmabuf, NVIDIA)
             │
             ▼
   ┌─────────────────────────────┐
   │ kasane — the GPU pipe        │   crates/kasane, beside crates/nuri
   │  ash  ── the ONE unsafe seam │   VK_EXT_external_memory_dma_buf
   │   │                          │   VK_EXT_image_drm_format_modifier
   │   ▼ safe typed surface       │
   │  wgpu (via create_device_    │   ← so garasu's pipelines, text and
   │        from_hal on OUR       │     kentou determinism are REUSED
   │        ash device)           │     rather than rebuilt
   └─────────────────────────────┘
             │
   rouka::choose  ── the router, per surface per frame
             │
   ┌─────────┴──────────┐
   ▼                    ▼
 GPU pipe            CPU pipe (nuri, unchanged)
 client surfaces     bar + chrome
 scaling, effects    tiny damage rects
```

**Reuse / colocate / redistribute**, in the operator's terms:

- **Re-use** — `garasu` for pipelines/text/shaders/`kentou`; `nuri` as the CPU
  pipe *and* as the template (it already implements the whole smithay
  `Renderer` trait surface, which proves the surface is implementable in-house);
  `rouka` for the route type.
- **Colocate** — `crates/kasane` in omoya's workspace, beside `crates/nuri`.
  **Not a new repo.**
- **Redistribute** — published like `nuri`. Nothing in the Rust ecosystem ships
  a Vulkan-backed smithay renderer with dmabuf interop; this is a genuine fleet
  primitive, not an omoya detail.

### The routing policy is tatara-lisp, not branches

Which work goes down which pipe is **data**:

```lisp
(defkasane seat
  (pipe gpu :import dmabuf :modifiers tiled
            :for (client-surface scaling effects))
  (pipe cpu :import shm    :modifiers linear
            :for (bar chrome tiny-damage))
  (fallback cpu :when (no-loader no-presentable-adapter)))
```

This is not symmetry for its own sake. The bar and chrome are small, text-heavy,
damage-tracked and already cached as ARGB buffers — a GPU round-trip for a 24 px
titlebar costs more than the copy. Client surfaces are the opposite. `rouka`
already carries the honest unit to decide on: `Cost { cpu_bytes_per_frame }`.

---

## 6. Milestones

Each has a done-predicate that is a **measurement**, not a feeling.

| | milestone | done-predicate |
|---|---|---|
| **M0** | Import one linear dmabuf as a `VkImage` via `ash` and read a pixel back | a test asserts the pixel, RUN ON llvmpipe so CI covers it |
| **M1** | Import a **tiled** dmabuf (`VK_EXT_image_drm_format_modifier`), NVIDIA | same test green on plo against a real client buffer |
| **M2** | `impl Renderer + Frame + ImportDma + ImportMem + Bind<Dmabuf> + ExportMem + ArmFlush` for `Kasane` | omoya's existing generic loop accepts it with **no change to `drm.rs`** — if `drm.rs` needs editing, the seam was wrong |
| **M3** | Selection + typed fallback | loader absent / no presentable adapter ⇒ nuri, with a red-run proving the fallback path |
| **M4** | `rouka::choose` wired as the router; `(defkasane …)` authors the policy | `route_label` reports `gpu-composited` for a client surface and `cpu-readback` for the bar, **in the same frame** |
| **M5** | Scanout | page-flip from a GPU-composited buffer; `Cost::is_zero_copy()` true for a client surface |

**M2's done-predicate is the load-bearing one.** If adding kasane requires
editing `drm.rs`, the renderer abstraction was not actually the seam it appears
to be, and that is worth discovering at M2 rather than M5.

---

## 7. Tier-honest ledger

<!-- tier-ledger -->

| bad state | how it is cornered | tier |
|---|---|---|
| a renderer that cannot be screenshot drives the seat | `ExportMem` in the generic bound — a renderer without it does not compile | truly-unrep (shipped) |
| a renderer silently gets a no-op flush plan and takes a full copy per frame | `ArmFlush::arm_flush` has no default body; every impl must decide | truly-unrep (shipped 2026-09-03) |
| the advertised dmabuf formats disagree with what the renderer can texture | the global is built from `renderer.dmabuf_formats()`, never a hand list | truly-unrep (shipped) |
| promising a capability actually served by a CPU readback | `rouka::Advertisement` has no public constructor (`E0603`) | truly-unrep (shipped) |
| a CPU blitter reads a tiled buffer and paints structured noise | `NuriRenderer::accepts` refuses non-linear modifiers at the import boundary | parse-time-rejected (shipped) |
| the Vulkan loader is absent on a machine | typed fallback to nuri | **not built** — M3 |
| the router sends bar text through a GPU round-trip that costs more than the copy | `rouka::choose` over `Cost`, authored in tatara-lisp | **not built** — M4 |
| unsafe spreads beyond the ABI seam | one `ash` seam, safe surface above it | **not built** — M0, and the thing to hold the line on |

★ **Nothing in §5–6 is built.** The shipped rows above are seams that already
existed and are what make this tractable; every kasane row is DESIGN.

---

## 8. The name

`kasane` (重ね) — *layering*. Compositing is layering, and it pairs with `nuri`
(塗り, *coating*) in one craft metaphor family, as the naming law requires.

★ **Corpus-checked 2026-09-03, and the check earned its keep.** The first
candidate was `yakitsuke` (焼き付け, *burning-in*), cleared by reasoning — and
`pleme-io/yakitsuke` is a **live repo** with its own `src/` and `CLAUDE.md`.
That is exactly the silent failure `theory/NAMING.md` records: a word is free
only when the CORPUS says so. `kasane` and `utsushi` were then checked across
1202 repos; neither names a pleme-io repo.
