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
| keymap translation (`libxkbcommon`) | **NOT SCOPED** *(when this table was written; superseded 2026-08-20 — see the second ledger, where it is built)*. XKB is a real format with real complexity and no fleet primitive. Naming it as unscoped was the honest move; a plan that quietly assumed it would fall out is the round-up. | only-mitigated (C6) |
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

> **Done, 2026-08-20 — and the order above was right for the wrong reason.**
> This was ranked last because XKB is a large spec. The actual obstacle was
> never the spec: it was that smithay re-exports `xkbcommon` in its public API
> with nothing gating it, so no feature choice could drop it. What unblocked it
> was a packaging seam, not a parser — `[patch.crates-io]` matches on package
> *name*, so replacing the dependency leaves smithay untouched. And the spec
> turned out not to need implementing: a keymap is **emitted** for clients, who
> compile it themselves, so hairetsu writes XKB text and never parses it.
> **Read the obstacle before ordering the work by apparent size.**

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


---

# The naturalize ledger — "no C involved", measured

> Operator directive, 2026-08-20: *"literally the only thing that should have C
> left is the kernel"*, then *"so we have no C involved"*.

## ★ The floor, named before the plan rather than discovered at its end

**Rust's `std` on Linux is implemented over libc.** The only libc-free target,
`x86_64-unknown-linux-none`, ships **no `std`** — measured on rio, not assumed:

```
$ cargo build --target x86_64-unknown-linux-none
error[E0463]: can't find crate for `std`
```

A Wayland compositor needs threads, files, sockets and an allocator; smithay,
calloop and wayland-server all require `std`. So "zero C" for this program means
`#![no_std]` plus a reimplementation of std's Linux layer over raw syscalls —
for us **and** for every crate we consume.

This is a **world fact**, not one of our abstractions, which is the test the
fleet's own doctrine sets for whether a limit is ours to dissolve. It gets
typed as a ceiling rather than argued with.

**So the honest destination is: kernel + libc, and nothing else.** Every C
library and every C daemon above that line goes.

<!-- tier-ledger -->

| X capability | pleme-io realization | tier |
|---|---|---|
| software rasterization (`libpixman`) | NET-NEW: `nuri`, 485 lines, **zero dependencies**, 11 green tests + a smithay `Renderer`/`Frame` adapter. The one true naturalize — it wraps no kernel interface, it is arithmetic. | only-mitigated (C2) |
| input transport (`libinput`, `libudev`) | NET-NEW: `evdev_backend`, kernel ioctls via the pure-Rust `evdev` crate (bitvec/cfg-if/libc/nix, **no `-sys`**). Wired as `--input evdev`. | only-mitigated (C2) |
| input **policy** (accel, tap, gestures) | **ABSENT.** libinput's real value, and not reimplemented. Named so the swap is not mistaken for parity. | only-mitigated (C6) |
| seat/device arbitration (`libseat`) | SHIPPED-composition: `logind.rs` over zbus, proven on vkms with a real seat session — **but logind is a C daemon**, so this trades a linked library for an out-of-process one. | only-mitigated (C2) |
| session **without any C daemon** | NET-NEW: `DirectSession` over `VT_SETMODE`/`VT_PROCESS` + `DRM_IOCTL_SET_MASTER`/`DROP_MASTER`. **NOT BUILT.** This is what actually removes logind. | only-mitigated (C6) |
| buffer allocation (`libgbm`) | dumb buffers are already the runtime path; the `.so` is a smithay compile-time gate on `DrmCompositor`. Removing it means replacing 4426 lines of damage tracking and plane assignment. **NOT BUILT.** | only-mitigated (C6) |
| keymap translation (`libxkbcommon`) | NET-NEW: `hairetsu` (配列) + a `[patch.crates-io]` replacement of the `xkbcommon` crate. **smithay is unmodified** — cargo patches by package name, so the dependency is swapped underneath it. 43 green tests. **Serves `us` only, and REFUSES any other layout** rather than substituting. | only-mitigated (C2) |
| the launcher (`makeWrapper` bash) | **RETIRED — deleted, not rewritten.** It existed to put five C libraries on `LD_LIBRARY_PATH`; nothing links them now, so there is no search path to set and no script to generate. `bin/omoya` is the ELF binary. | truly-unrep |
| the Rust runtime (`libc`) | **THE FLOOR.** No `std` without it; `linux-none` is `no_std`. Measured above. | only-mitigated (C6) |

## The measurement, on the artifact (rio, `6a43e07`)

Two censuses, because either one alone can lie. `ldd` misses a `dlopen`; the
closure catches what `ldd` cannot see.

```
$ ldd .../rust_omoya-0.1.14/bin/omoya
    libc.so.6   libgcc_s.so.1   libm.so.6   linux-vdso.so.1   ld-linux-x86-64.so.2

$ strings -a .../bin/omoya | grep -oE 'lib[a-zA-Z0-9_.+-]*\.so[0-9.]*' | sort -u
    libc.so.6   libgcc_s.so.1   libm.so.6

$ nix path-info -r .../rust_omoya-0.1.14 | grep -icE 'wayland|xkb|libinput|pixman|seatd|gbm|mesa'
    0                                   # closure is 5 paths, was 107
```

Before this work the same binary linked nine: `libc libgbm libgcc_s libinput
libm libpixman-1 libseat libudev libxkbcommon`.

**The closure is why the wrapper had to go.** `ldd` was already clean while
`nix path-info -r` still listed wayland, pixman, libinput, mesa-libgbm and
libxkbcommon, because the wrapper kept naming them. A library nothing links but
the closure still carries is a claim that has not been made good on.

**And `strings` is why the nested backend is now off by default.** winit pulls
`xkbcommon-dl`, which `dlopen`s libxkbcommon at *runtime* — a binary carrying it
reports a clean `ldd` while loading the very library this tree replaced. Same
shape as `ash` dlopening libvulkan, and it makes the census lie in the
flattering direction, which is worse than an honest link.

## What this ledger says plainly

**Six C libraries are gone from the shipped compositor, measured on the
artifact by two independent censuses.** What remains below the kernel is
`libc` + `libm` + `libgcc_s` — the Rust runtime floor named at the top of this
document before the work started, not discovered at its end.

**Three things are still true and are not rounded away:**

- **logind is still a C daemon.** `libseat.so` is gone from this binary, but
  the seat is still arbitrated by a C process over D-Bus. That trades a linked
  library for an out-of-process one; it does not remove C from the *system*.
  `pending-omoya-direct-session` is the row that would.
- **`hairetsu` is not XKB.** One layout, no keymap parsing, no compose, no
  layout switching. It refuses what it cannot serve, which makes the gap loud
  rather than silent — but a refusal is still a gap.
- **input policy is absent, not ported.** Acceleration, tap-to-click and
  gestures were libinput's real value and were not reimplemented.

"No C in the compositor binary" is now true and measured. "No C on the seat"
is not, and the two rows above are the distance between them.
