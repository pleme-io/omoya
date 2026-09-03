# kasane (重ね) — the GPU pipe, and the router that decides which pipe

> **Status: DESIGN. No code exists.** Measured 2026-09-03. Every capability
> claim below was probed on plo's real hardware or read from omoya's source;
> nothing here is inferred. Re-measure before acting on any number.

---

## 0. The problem, measured — ★ CORRECTED 2026-09-03

> **This section was WRONG in its central claim, and the milestones rested on
> it.** It said *"NVIDIA does not present linear"*, and made M1 "tiled
> modifiers". Both are refuted by omoya's own source, which I did not read
> before writing them. The original text is kept below the correction because
> the error is instructive: it is a plausible story that explains the symptom
> and is not what is happening.

Every GPU client on plo renders on the **CPU**, on `llvmpipe`. That much is
true and unchanged.

**The real chain, measured against the running compositor:**

1. omoya does **not advertise `zwp_linux_dmabuf_v1` at all**. `wayland-info`
   against the live seat lists 11 globals and it is not among them; the
   journal says `zwp_linux_dmabuf_v1 WITHHELD`. The gate is an environment
   variable — `drm.rs:786`, `if std::env::var_os("OMOYA_ADVERTISE_DMABUF")` —
   and it is unset in `/proc/<pid>/environ`.
2. NVIDIA's Wayland WSI needs `wl_drm` or `zwp_linux_dmabuf_v1`. omoya offers
   neither, so `vulkaninfo` run as a client of that seat enumerates both
   physical devices but lists **only llvmpipe under Presentable Surfaces**.
   lavapipe's WSI works over `wl_shm`, which omoya does offer.
3. **Modifiers never enter the picture.** Clients get `wl_shm` (AR24/XR24).

**And linear presents fine.** `DRM_FORMAT_MOD_LINEAR` is in `IN_FORMATS` on
all 12 of plo's KMS planes for ARGB8888/XRGB8888 — every primary, every
overlay, and it is the *only* modifier the four cursor planes accept.
`drm.rs:747` already recorded this: *"measured on plo, the RTX 3070 exposes
DRM_FORMAT_MOD_LINEAR for B8G8R8A8 as a single-plane, exportable, importable
COLOR_ATTACHMENT image, and the exported fd mmaps read/write."* omoya also
scans out through `DumbAllocator`, whose buffers are linear by construction —
if linear could not present, the seat would be blank.

**Why the global is off is legitimate, and is the actual constraint.**
`nuri_renderer.rs`'s `import_dmabuf` maps the client buffer and `to_vec`s it:
a CPU readback of VRAM across PCIe. Measured at `drm.rs:764`: `gather_us
693 952` against `frame_us 3 825`. Advertising a capability served that badly
is worse than not advertising it, and withholding it was the right call.

★ **So the target is not modifiers. It is a zero-copy import — an
`ImportDma` that hands smithay a `TextureId` the GPU samples in place —
after which the global can be advertised honestly.**

### The original text, kept because the error is the lesson

> *"omoya composites on the CPU, so it advertises linear-modifier dmabuf only
> … NVIDIA does not present linear. So no hardware adapter can present to an
> omoya surface."*

Step 2 is false (there is no advertisement at all, so it was never
exercised) and step 3 is false (linear presents on every plane). The story
was coherent, matched the symptom, and sent the roadmap at the wrong problem
— which is why a claim about how two existing components relate gets read
from source on both sides before it lands in a doc.

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

## 6. Milestones — ★ REORDERED 2026-09-03

The original order (M0 read a pixel back, M1 tiled modifiers) was built on the
refuted premise in §0 and aimed at the wrong problem.

★ **The risk that reordering removes, stated plainly: M0's and old-M1's
done-predicates were READBACK MACHINERY, and readback is the defect.** Both
build exactly the CPU-copy path that took the global off the wire. kasane
could have been built through M5 with every done-predicate green and the
desktop still on llvmpipe, because nothing before the zero-copy import causes
the protocol global to be published. The global is env-gated, not
renderer-gated — so "select a GPU renderer and NVIDIA can present" was false.

| | milestone | done-predicate (a MEASUREMENT) |
|---|---|---|
| **M0** ✅ | dmabuf round-trips through Vulkan in pure Rust | shipped — but see the note below on what it does and does not prove |
| **M1** | Import a real client dmabuf — tiled, device-local — as a **sampled** `VkImage`, composite from it, never touch it with the CPU | With a GPU client on plo: `gather_us < 5 000` (baseline **693 952**) **and** `Cost::cpu_bytes_per_frame == 0`. Not a pixel. |
| **M2** | `impl Renderer + Frame + ImportDma + ImportMem + Bind<Dmabuf> + ExportMem + ArmFlush` for `Kasane` | `git show --stat <commit> -- crates/omoya/src/drm.rs` is **empty**. If drm.rs needs editing the seam was wrong. |
| **M3** | The dmabuf global becomes a typed capability on the renderer bound, not a shell variable | `wayland-info` lists `zwp_linux_dmabuf_v1` under kasane and does **not** under nuri, with `OMOYA_ADVERTISE_DMABUF` unset in both runs. `vulkaninfo` then names the RTX 3070 under Presentable Surfaces. |
| **M4** | Device selection by DRM node + typed fallback | kasane binds the physical device whose `VkPhysicalDeviceDrmPropertiesEXT` major:minor matches the `DrmDeviceFd`'s `st_rdev` (226:1 / 226:128 on plo). Red run: hide the loader → nuri, fallback proven rather than inferred. |
| **M5** | Scanout | page-flip from a GPU-composited buffer; `Cost::is_zero_copy()` true for a client surface. |

★ **What M0 actually proves, restated honestly.** It proves the external-memory
machinery works end to end in pure Rust: a real kernel dmabuf fd, a real
`vkBindImageMemory` of imported memory, on both NVIDIA and llvmpipe. That is
worth having and it is the foundation M1 builds on. It does **not** prove
anything about compositing, and its `export_linear` half is test scaffolding
— two references, one of them inside `#[cfg(test)]`. Do not let it drive the
design of M1.

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
