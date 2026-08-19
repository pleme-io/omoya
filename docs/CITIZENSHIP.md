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
