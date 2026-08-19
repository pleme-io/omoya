# omoya (母屋)

> 母屋 — the main house, the structure the entrance ([mukae](https://github.com/pleme-io/mukae)) opens into.
> The pleme-io-native Wayland compositor: greeter, desktop and lock as **three
> modes of one process**.

**Status: M0.** `omoya-spec` — the typed mode machine — is shipped and green.
There is no compositor yet: no smithay, no Wayland, no pixels. See
[`theory/OMOYA.md`](https://github.com/pleme-io/theory/blob/main/OMOYA.md) for
the destination, the foundation decision, and the phase ladder.

## What exists

| crate | what | deps |
|---|---|---|
| `omoya-spec` | `Entrance`/`Session`/`Locked` typestate, `AuthProof`, `DrmMaster`, the `CompositorEnv` seam, `MockEnv` | `thiserror`, `tracing` — **and nothing else** |

That dependency set is the point: the mode machine's invariants are checkable on
a Mac, in CI, with no GPU and no seat. A login manager first exercised on the
machine it can lock you out of is a bad plan.

## The invariants, and how each is enforced

| illegal state | mechanism | proof |
|---|---|---|
| leaving `Locked` without authenticating | `unlock` takes an `AuthProof` by value | `tests/ui/unlock_without_proof.rs` → E0061 |
| one authentication unlocking twice | `AuthProof` is not `Clone` and is consumed | `tests/ui/reuse_auth_proof.rs` → E0382 |
| forging an `AuthProof` | private field; only `CompositorEnv::authenticate` mints one | `tests/ui/forge_auth_proof.rs` |
| two DRM masters | `DrmMaster` is move-only with a private field | `tests/ui/forge_drm_master.rs` |
| a fourth mode | `ModeState` is sealed | `tests/ui/foreign_mode_state.rs` → E0277 |

See [`docs/VERIFICATION.md`](docs/VERIFICATION.md) for the measured error codes
and the recorded red run.

## Building

`cargo test` — no system dependencies. Later phases need Linux, a seat and a
DRM device; M0 needs none of them.

## License

MIT.
