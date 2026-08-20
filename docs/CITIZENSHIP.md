# The seat's citizenship ledger — what is still not ours, and what to do about each

> **Census date 2026-08-19.** Measured on plo, against the running binaries, by
> `ldd`. Re-measure before acting: this is a dated claim and the org's standing
> rule says so.

## The question

> *"if we /naturalize everything why do we need all this .so — everything
> should be rust"*

Correct, and the answer is sharper than "rewrite them all." **Most of these are
not naturalize targets at all.** The `naturalize` skill's own waiver names the
distinction: *a standard we must speak on the wire → speak the wire, own the
executor (magma's posture, not a naturalize)*. A C library that wraps a kernel
interface is a **convenience**, and the interface underneath it is a **wire**.

## The measurement

`ldd` on the running compositor, beyond libc:

| `.so` | smithay feature that pulls it | crate |
|---|---|---|
| `libpixman-1.so.0` | `renderer_pixman` | `pixman` |
| `libgbm.so.1` | `backend_gbm` | `gbm` |
| `libinput.so.10` | `backend_libinput` | `input` |
| `libudev.so.1` | `backend_udev` | `udev` |
| `libseat.so.1` | `backend_session_libseat` | `libseat` |
| `libxkbcommon.so.0` | (transitive, keymap handling) | `xkbcommon` |

**★ Two facts that decide the whole plan.**

**(1) `backend_drm` pulls NO `.so`.** `drm` + `drm-ffi` are pure Rust speaking
ioctls. Mode-setting on a real CRTC — the most kernel-adjacent thing this
compositor does — is *already* done with zero C. That is not an aspiration; it
is what shipped and was witnessed on plo. Every remaining library sits on an
interface of the same kind.

**(2) omoya's own source uses NONE of them directly.** `grep -rn gbm crates/omoya/src`
returns nothing. `backend_gbm` is on only because smithay gates `DrmCompositor`
behind it. So these are not choices this repo made; they are **smithay's**
choices, inherited. That matters for the plan: the lever is smithay's feature
surface and what we ask of it, not omoya's code.

## What is actually underneath each

| library | what it wraps | verdict |
|---|---|---|
| `libgbm` | DRM buffer allocation. **We scan out on DUMB BUFFERS** (`DRM_IOCTL_MODE_CREATE_DUMB`), which are pure ioctl and need no GBM. | **wire** — speak it |
| `libinput` | evdev: `/dev/input/event*` + `EVIOC*` ioctls | **wire** — speak it |
| `libudev` | netlink + `/sys` enumeration | **wire** — speak it |
| `libseat` | a socket protocol to seatd, or D-Bus to logind — and **we already speak logind D-Bus** (`mukae-native::logind`, proven live 2026-08-19) | **wire** — speak it |
| `libxkbcommon` | the XKB keymap **format** plus a state machine | **format + library** — the hard one |
| `libpixman` | pure computation. No kernel, no protocol, no format. | **★ the ONE true guest** |

## The ledger

<!-- tier-ledger -->

| X capability | pleme-io realization | tier |
|---|---|---|
| software rasterization (`libpixman`) | **compose, don't build**: `engawa` (typed render-graph IR) + `garasu`'s headless paint plane already exist and have no consumer on this seat. PENTE's rule — *ship a consumer, or don't ship* — makes omoya that consumer. NET-NEW is only the DRM-surface dispatcher. | only-mitigated (C2) |
| DRM/KMS mode-setting | SHIPPED-composition: `drm`/`drm-ffi`, pure-Rust ioctls, **already zero `.so`** and witnessed on plo | parse-time-rejected |
| buffer allocation (`libgbm`) | dumb buffers are `DRM_IOCTL_MODE_CREATE_DUMB` — already what we use. The `.so` is a smithay feature gate on `DrmCompositor`, not a call we make. | only-mitigated (C2) |
| seat/device arbitration (`libseat`) | SHIPPED-composition: `mukae-native::logind` speaks `org.freedesktop.login1` over zbus (pure Rust D-Bus). `TakeDevice`/`TakeControl` are methods on the same bus we already reached. | only-mitigated (C2) |
| input devices (`libinput`, `libudev`) | NET-NEW: evdev ioctls + netlink are kernel wires of the same class as DRM. `awase` already owns the TYPED key layer; this is the transport under it. | only-mitigated (C2) |
| keymap translation (`libxkbcommon`) | **NOT SCOPED.** XKB is a real format with real complexity and no fleet primitive. Naming it as unscoped is the honest move; a plan that quietly assumed it would fall out is the round-up. | only-mitigated (C6) |
| the launcher (`makeWrapper` bash) | omoya's entry point is a generated **bash script** setting `LD_LIBRARY_PATH`. In a repo whose law is NO SHELL, the compositor is launched by shell. Fixed by RPATH (substrate learned this 2026-08-19) rather than a wrapper. | only-mitigated (C2) |

**Every row is `only-mitigated` except DRM, and that is the point of writing the
table.** Nothing here is retired yet. "We measured what is foreign" is true;
"we replaced it" is true of nothing on this list today.

## Ceilings, named

- **C2 (external-world observation)** on most rows: whether a kernel ioctl
  sequence is *equivalent* to what libinput/libgbm does is not a compile-time
  property. It is checked by running both against the same device and
  comparing — a differential, not a type.
- **C6** on XKB: the format's semantics (compose sequences, layout switching,
  level-5 modifiers) are a spec-sized surface. Claiming a tier before reading
  the spec would be inventing one.

## What NOT to do

- **No new repo for a renderer.** `engawa` + `garasu` exist. PENTE's warning
  about creating "the 22nd palette home" applies exactly.
- **No naturalize of smithay.** It is Rust, it is a library we consume, and
  replacing it is a different and much larger decision than removing six `.so`.
  The lever is which features we ask it for.
- **No claim that any of this is done.** See the ledger.

## The order, and why

1. **`libpixman` first** — the one true guest, and the one with a fleet
   replacement already built and consumer-less. Highest leverage per unit work,
   and it converges with the graphics-spine plan rather than competing with it.
2. **`libseat`** — smallest, because the bus is already spoken.
3. **`libgbm`** — may fall out of (1): a renderer that owns its scanout target
   changes what `DrmCompositor` is needed for.
4. **`libinput`/`libudev`** — a real build, same class as the DRM work that
   already succeeded.
5. **`libxkbcommon`** — last, and only after reading the spec.

---

## P2 — the exact trait surface a rasterizer must satisfy

Measured 2026-08-19 against smithay 0.7 and omoya's own call sites. Recorded
here so the next person does not re-derive it.

**★ omoya never calls a raster op directly.** No `Renderer::render()`, no
`Bind::bind()`, no `Frame` draw method appears anywhere in `crates/omoya/src`.
Everything is driven inside `DrmCompositor::render_frame`, which dispatches
through `RenderElement::draw` (`smithay .../renderer/element/surface.rs:383-404`).
omoya's entire direct use of pixman is **two calls**: `PixmanRenderer::new()`
(`drm.rs:355`) and `renderer.dmabuf_formats()` (`drm.rs:367`).

**★ The whole per-frame drawing vocabulary is THREE ops** — `clear`,
`draw_solid`, `render_texture_from_to`. `render_texture_at` is a trait default.
That is the real size of "replace pixman".

### What must be implemented

| trait | why |
|---|---|
| `RendererSuper` | 4 assoc types: `Error`, `TextureId: Texture`, `Framebuffer<'b>: Texture`, `Frame<'f,'b>: Frame` |
| `Renderer` | `context_id`, `downscale_filter`, `upscale_filter`, `set_debug_flags`, `debug_flags`, `render`, `wait`, `cleanup_texture_cache` |
| `Frame` | the three raster ops + `context_id`, `transformation`, `wait`, `finish` |
| `Texture` | on **both** `TextureId` and `Framebuffer` |
| `ImportMem` + `ImportMemWl` | client SHM buffers |
| `ImportDma` + `ImportDmaWl` | `ImportDmaWl` is an empty marker |
| `Bind<Dmabuf>` | **`Dmabuf`, not `DumbBuffer`** — see the chain below |

**`ImportEgl` is NOT needed.** `ImportAll` is a blanket impl and which one
applies is decided by the `use_system_lib` feature; omoya does not enable it,
so the applicable blanket is
`impl<R: Renderer + ImportMemWl + ImportDmaWl> ImportAll for R`
(`renderer/mod.rs:700-706`). Optional, and only if `drm::capture` is revived:
`ExportMem` + `TextureMapping`.

### The chain the replacement must fit into

`DumbAllocator` → `DumbBuffer` → **dmabuf** → renderer. `DumbBuffer` implements
`AsDmabuf` (`allocator/dumb.rs:104`) and `DrmDeviceFd` is its own
`ExportFramebuffer` (`drm/exporter/dumb.rs:26`), so `DrmCompositor` performs the
export itself and hands the renderer a `Dmabuf`. Pixman then **mmaps plane 0**
and rejects multi-plane or non-Linear modifiers
(`renderer/pixman/mod.rs:736,746-748`).

So the rasterizer's job is narrower than "a 2D graphics library": **blit with a
source rect, a transform and alpha, into an mmap'd single-plane Linear buffer.**

Formats: the CRTC is offered `[Argb8888, Xrgb8888]` (`drm.rs:366`); pixman
advertises 13 fourccs, **all at modifier Linear** (`pixman/mod.rs:44-63`,
`:1184-1191`).

### Verification available, and better than expected

**`vkms` — the virtual KMS driver — is in the running kernel** (confirmed on
rio, 6.12.93: `drivers/gpu/drm/vkms/vkms.ko.xz`). A `nixosTest` can boot with
it, run `omoya --backend drm` against a virtual card, drive a scripted client
and assert a golden hash — the whole scanout path, in CI, with no hardware and
no risk to plo. That is the gate for this phase.
