# Smoothness — why the seat is not smooth, measured

> Written 2026-08-21 after three fixes that were each a real bug and each
> barely moved the number. Every figure here was taken on **plo** (1920×1080,
> RTX 3070, 16 cores, otherwise idle at load 1.19), not in the VM.

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

## The plan, in leverage order

### 1. shm damage, for real — `RwLock`, not `get_mut`
Interior mutability so the damage-only path can write into the cached
allocation regardless of who else holds a reference. The lock is taken once
per texture per **frame**, at the top of a blit, never per pixel.

**Expected**: typing copies one row instead of 8 MB. Also expected to help
the **mouse**, because pointer motion produces no client commit at all — so
with import fixed, a mouse move costs a small damage repaint and nothing
else.

### 2. Hardware cursor plane — `pending-omoya-planes`
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

### 4. vblank-driven loop — `pending-omoya-vblank`
Today a calloop timer polls and early-returns when a flip is pending. Frame-
perfect pacing means rendering **on** the `DrmDeviceNotifier` vblank event.
This is the smallest win of the four and the last one worth doing.

## Rule this file exists to enforce

**Measure after each step before starting the next.** Three of the four
changes above were justified by a plausible story about where time goes, and
three stories were wrong. `frame_us` against wall-clock is what settled it —
not a profile, not reasoning about the code.
