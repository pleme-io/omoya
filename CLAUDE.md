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

**[`docs/SMOOTHNESS.md`](./docs/SMOOTHNESS.md)** carries the measurements. The
three findings that cost the most to reach, so nobody re-derives them:

- **A protocol global is a PROMISE about what we serve WELL.** Advertising
  `zwp_linux_dmabuf_v1` made the seat 400× slower: mado switched to it because
  it appeared, and `import_dmabuf` reads the client's buffer — which for a GPU
  client is a CPU readback of VRAM across PCIe. It is withheld behind
  `OMOYA_ADVERTISE_DMABUF` until direct scanout means the buffer is never read.
- **`PREFERRED` picks the RESOLUTION, never the RATE.** EDID's preferred timing
  on a high-refresh panel is routinely the 60 Hz compatibility descriptor. plo
  is a 360 Hz display and we were driving it at 60, with latency as the only
  symptom. `scanout` now takes the fastest mode at the preferred resolution,
  ranked by the DERIVED rate — `vrefresh` is 0 on this panel and would rank
  every mode equally. The `modes` leaf publishes the list, selection starred.
- **The frame decision is `mekuri`'s, and the verdict PRODUCES the permission.**
  `crates/mekuri` — `Verdict::Skip` carries no `Pass`, and the composite runs
  inside `pass.spend`, so "decided to skip, drew anyway" has no shape to write.
  Extracted because mado had the identical defect with the operands reversed.
  It is its own crate now — [`pleme-io/mekuri`](https://github.com/pleme-io/mekuri),
  consumed from crates.io — so **fix the decision there, not here**. The GPU
  twin is `madori::RenderCallback::needs_frame`, asked before the swapchain
  acquire; mado implements it.

**The rule the file exists to enforce: measure after each step before starting
the next.** Three fixes in a row were justified by a plausible story about
where the time went, and all three stories were wrong.

## Verification

[`docs/VERIFICATION.md`](./docs/VERIFICATION.md).

## Traps recorded elsewhere, worth knowing here

- **The entry point is a generated bash wrapper** setting `LD_LIBRARY_PATH` —
  in a repo whose law is NO SHELL. substrate's `library-workspace` learned
  RPATH on 2026-08-19, which is the fix.
- **Nord is encoded per backend.** Linear values written into a non-sRGB
  framebuffer render as near-black; `theme::background_for_surface` takes the
  surface's `srgb` flag for exactly this reason.
