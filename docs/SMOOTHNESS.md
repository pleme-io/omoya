# Smoothness — why the seat is not smooth, measured

> Written 2026-08-21 after three fixes that were each a real bug and each
> barely moved the number. Every figure here was taken on **plo** (1920×1080,
> RTX 3070, 16 cores, otherwise idle at load 1.19), not in the VM.
>
> **★ RESOLVED THE SAME DAY. The diagnosis below is kept because it is the
> record of how the answer was found, and of three confident wrong turns —
> but every number in it is superseded. Read "What it actually was" first.**

## The measurements that matter

| reading | value | what it rules out |
|---|---|---|
| `frame_us` | **4 009 µs** | compositing. `render_output` takes 4 ms |
| `blit_fast` / `blit_slow` | 122 216 / **0** | the blit. Its fast path always fires |
| omoya CPU | **99 %** of one core | waiting. It is computing |
| gdb samples | **all** in `__memmove_evex_unaligned_erms` | everything except copying |
| render ticks | **1.4 / s** | — |

Four milliseconds of compositing cannot produce a 700 ms frame. The cost is
**outside** `render_output`, in the element-gather phase — which is where
texture import happens.

## The shape of the problem

```
mado renders on the RTX 3070
  → hands omoya an 8 MB CPU buffer (wl_shm)
  → nuri memcpys it with the CPU
  → into a dumb scanout buffer
```

The GPU is idle while a core does memory copies. **This is not a slow
implementation, it is the wrong shape**, and optimising the copy has a
ceiling we are already near.

## Three real bugs that were not the answer

Recorded because the pattern is instructive: each was genuine, each was
fixed, and none of them was *the* cost. The tell was identical every time —
**the number did not move.**

1. **`vrefresh` = 0 → 1 Hz scheduling.** `drmModeModeInfo.vrefresh` is
   optional and this panel leaves it 0; `.max(1)` turned "unknown" into "one
   frame per second". Real, fixed — and an 8 MB memcpy per frame does not
   care what the timer says.
2. **Per-pixel blit.** A transform map-back, two divisions, a bounds-checked
   lookup and an offset, ~2 M times per frame. Real, fixed — and it sat
   *behind* the import in the same frame. 1.4 → 2.2 fps.
3. **shm damage ignored, attempt 1.** `import_shm_buffer`'s damage parameter
   was named `_damage`. Fixed by caching the texture and copying only damaged
   rows — which **silently never engaged**, because it used `Arc::get_mut`
   and smithay holds the texture across frames, so `get_mut` always returned
   `None` and every commit fell through to the full copy. Measured after:
   unchanged.

## What it actually was — three causes, none of them the first suspect

| | before | after |
|---|---|---|
| under load | 1.4 fps | **111 fps** |
| `gather_us` | 693 952 µs | **4 µs** |
| `frame_us` | 4 009 µs | 4 194 µs (unchanged — it was never the problem) |
| display mode | 1920×1080@60 | **1920×1080@360** |
| omoya idle | 38.2 % of a core, 0 frames | ~0 |
| mado idle | 50.7 % of a core, 0 frames | ~0 |

**1. We advertised a capability we served badly.** The `zwp_linux_dmabuf_v1`
global landed that morning; mado switched to it *because it appeared*, and
`import_dmabuf` `to_vec()`s the client's buffer — which for a GPU client lives
in VRAM, so the copy is a CPU readback across PCIe. 8 MB, every frame, 694 ms.
The global is withheld behind `OMOYA_ADVERTISE_DMABUF` until direct scanout
means the buffer is never *read*. A protocol global is a promise about what
the compositor does **well**; the client cannot see the implementation.

**2. We asked a 360 Hz panel for 60 Hz.** `scanout` took the connector's
`PREFERRED` mode. That is right about *resolution* and silently wrong about
*rate* — EDID's preferred timing on a high-refresh panel is routinely the
60 Hz compatibility descriptor, with the fast modes in the extension blocks.
plo advertises `1920x1080@60 @360 @300 @240 @144 @120`, preferred first. Now:
resolution from `PREFERRED`, then the fastest mode at that resolution, ranked
by the **derived** rate (`vrefresh` is 0 on this panel and would rank every
mode equally). Never trades pixels for hertz. The `modes` leaf publishes the
list with the selection starred.

**3. Both renderers decided not to draw, and drew anyway.** omoya composed a
full frame and asked about damage afterwards; mado computed the skip verdict,
counted it into `TOTAL_FRAMES_SKIPPED`, logged it, and fell through to a full
repaint (9 934 969 of 10 726 562 "skipped", none skipped). Same defect,
opposite operand order, never coordinated — so the decision was extracted as
**`mekuri`**, where the verdict *produces* the permission: `Verdict::Skip`
carries no `Pass`, and drawing takes one. The GPU-side twin is
`madori::RenderCallback::needs_frame`, asked before the swapchain acquire so
skipping the draw also skips the present — which is what mado's original
attempt could not express, and why it produced the shadow regression.

**Region damage was deliberately NOT extracted**: mado tracks 1-D row spans,
omoya 2-D rects. Same goal, different shapes; one type would fit neither.

## What is left

**`frame_us` — 4.2 ms — is now the ceiling** (~238 fps against a 2.78 ms
vblank). It is the CPU composite, and the way through it is **direct
scanout**: hand a fullscreen client's buffer straight to one of plo's 12 DRM
planes and touch no pixels at all. That also stays inside the no-C line —
`backend_drm` pulls zero `.so`. `pending-omoya-planes`,
`pending-nuri-dmabuf-zerocopy`.

Below this line is the original diagnosis, kept for the method.

## The plan, in leverage order

### 1. shm damage, for real — `RwLock`, not `get_mut` — ✅ DONE, and it was not the fix
The damage-only path landed and was correct. It changed nothing, because
`import_shm_buffer` **was never being called** — mado had moved to dmabuf.
`import_full: 0 / import_partial: 0` is what said so, and nothing else would
have.

Interior mutability so the damage-only path can write into the cached
allocation regardless of who else holds a reference. The lock is taken once
per texture per **frame**, at the top of a blit, never per pixel.

**Expected**: typing copies one row instead of 8 MB. Also expected to help
the **mouse**, because pointer motion produces no client commit at all — so
with import fixed, a mouse move costs a small damage repaint and nothing
else.

### 2. Hardware cursor plane — `pending-omoya-planes` — still open, still not urgent
The mouse became smooth without it, exactly as the "measure first" note
below predicted. Folded into the direct-scanout work.

`surface.planes().cursor` is available. A cursor on its own plane moves with
**zero** compositing: the CRTC scans it out separately and a move is a plane
position update, not a frame.

**Do this only if (1) does not already make the mouse smooth** — measure
first. A cursor plane is real work and (1) may subsume it.

### 3. Zero-copy dmabuf + direct scanout — `pending-nuri-dmabuf-zerocopy`
The structural fix. omoya already advertises `zwp_linux_dmabuf_v1`; if the
client hands over a GPU buffer, a single tiled or fullscreen window can be
**scanned out directly** with no per-frame work at all. Not "a fast copy" —
*no copy*.

The honest limit: nuri MAPS buffers, so it can only address `LINEAR`
modifiers. Direct scanout sidesteps that by not touching the pixels; mixed
scenes still need the CPU path.

### 4. vblank-driven loop — `pending-omoya-vblank` — still open
Now more attractive than when written: at 360 Hz the timer's error is a
larger fraction of a 2.78 ms period than it was of 16.7 ms.

Today a calloop timer polls and early-returns when a flip is pending. Frame-
perfect pacing means rendering **on** the `DrmDeviceNotifier` vblank event.
This is the smallest win of the four and the last one worth doing.

## Rule this file exists to enforce

**Measure after each step before starting the next.** Three of the four
changes above were justified by a plausible story about where time goes, and
three stories were wrong. `frame_us` against wall-clock is what settled it —
not a profile, not reasoning about the code.
