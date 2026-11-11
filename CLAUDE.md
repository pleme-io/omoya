# omoya (母屋) — the pleme-io-native Wayland compositor

The citizen half of plo's seat: DRM/KMS scanout, libinput through a libseat
session, Nord taken from `irodori` rather than a literal.

## Read this first when touching dependencies

**[`docs/CITIZENSHIP.md`](./docs/CITIZENSHIP.md) — the seat's foreign-dependency
ledger.** Every `.so` this compositor links, measured by `ldd` on plo, with a
tier-honest verdict per row.

Its two load-bearing findings, so nobody re-derives them:

- **`backend_drm` pulls NO `.so`.** `drm`/`drm-ffi` are pure Rust speaking
  ioctls, so mode-setting on a real CRTC is already done with zero C. Every
  remaining library sits on an interface of the same kind — which makes most of
  them **wires to speak**, not guests to rebuild.
- **This crate's own source uses none of them.** `backend_gbm` is on because
  smithay gates `DrmCompositor` behind it, not because we allocate a GBM
  buffer. The lever is smithay's feature surface, not code here.

Exactly one is a true guest: **`libpixman`** — pure computation, no kernel, no
protocol, no format. Its replacement already exists in the fleet (`engawa`'s
typed render-graph IR + `garasu`'s headless paint plane) and has no consumer on
this seat, so that work is compose-don't-build rather than a new repo.

## Smoothness — read before touching the render loop or the mode

**[`docs/SMOOTHNESS.md`](./docs/SMOOTHNESS.md)** carries the measurements and the
full findings; read it before you change a protocol global, the mode or the frame
decision. In short:

- A protocol global is a promise about what we serve well:
  `zwp_linux_dmabuf_v1` stays behind `OMOYA_ADVERTISE_DMABUF`.
- `PREFERRED` picks the resolution, never the rate: `scanout` takes the fastest
  mode at that resolution.
- The frame decision is [`mekuri`](https://github.com/pleme-io/mekuri)'s: fix it
  there, not here.

**Measure after each step before starting the next.**

## The daily seat

**[`docs/DAILY-SEAT.md`](./docs/DAILY-SEAT.md)** is the plan and the ledger for omoya as
a person's whole session: the blue world (`~/.config/omoya/omoya.b`, served by a child,
never on the render thread), lock, idle, sleep, notes and shots. Read it before you
touch `world.rs`, `session_lock.rs`, `notes.rs` or `screencopy.rs`.

## Verification

[`docs/VERIFICATION.md`](./docs/VERIFICATION.md).

## Traps recorded elsewhere, worth knowing here

- **The entry point is a generated bash wrapper** setting `LD_LIBRARY_PATH` —
  in a repo whose law is NO SHELL. substrate's `library-workspace` learned
  RPATH on 2026-08-19, which is the fix.
- **Nord is encoded per backend.** Linear values written into a non-sRGB
  framebuffer render as near-black; `theme::background_for_surface` takes the
  surface's `srgb` flag for exactly this reason.
