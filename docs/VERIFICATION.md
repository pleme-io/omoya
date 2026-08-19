# omoya-spec M0 — verification

Measured 2026-08-19, `rustc` from the fleet toolchain, aarch64-apple-darwin.

## Unit tests — 7/7

`the_full_transcript_entrance_session_locked_session` is M0's done-predicate: a
scripted run reaches every mode and the transition tape reads
`["entrance>session", "session>locked", "locked>session"]`.

The other six pin: bad credentials mint no proof; a compositor cannot start
without DRM master; **the master is acquired ONCE and survives every
transition** (four transitions, `master_acquisitions == 1`); fast user switching
returns to the entrance; one authentication authorizes exactly one transition;
and the mode names are stable because renaming one is an interface change.

## Seals — 5 trybuild cases, all committed with their `.stderr`

| case | measured error |
|---|---|
| `unlock_without_proof` | **E0061** — "this method takes 2 arguments but 1 argument was supplied" |
| `reuse_auth_proof` | **E0382** — "use of moved value: `proof`" |
| `foreign_mode_state` | **E0277** — "the trait bound `Kiosk: omoya_spec::sealed::Sealed` is not satisfied" |
| `forge_auth_proof` | **unnumbered** — "cannot construct `AuthProof` with struct literal syntax due to private fields" |
| `forge_drm_master` | **unnumbered** — "cannot construct `DrmMaster` with struct literal syntax due to private fields" |

★ **Two predictions were wrong and are corrected here rather than in a comment
nobody re-checks.** The two forge cases were predicted as E0063; they are an
UNNUMBERED error. The tier is unchanged — a hard compile error either way — but
the code is not what the design said, and a doc asserting E0063 would have sent
the next reader looking for the wrong string.

## The red run — and it is a GOOD seal, not a shifted one

`mukae`'s M0 recorded the trap this check exists for: *"A case testing the
symptom read as testing the cause."* So the most important seal was deliberately
broken and re-run.

**Break applied:** `Compositor::<Locked>::unlock` had its `proof: AuthProof`
parameter removed, so unlocking no longer required authenticating.

**Result:**

```
test tests/ui/unlock_without_proof.rs ... error
Expected test case to fail to compile, but it succeeded.

test illegal_states_do_not_compile ... FAILED
```

The case **compiles** once the requirement is gone, which is exactly what makes
this a **GOOD** seal in mukae's vocabulary: it tests the *mechanism* (the
signature demands a proof), not a symptom that some other error would mask.

The seal was restored and the suite re-verified green (7 unit + 1 compile_fail),
clippy pedantic clean.

## Not verified

Everything the compositor actually does. There is no compositing, no rendering,
no protocol object, no output, no input, and no DRM device anywhere in this
crate — by design, per `theory/OMOYA.md` §"What this crate deliberately does NOT
model". M0 says the *mode machine* is right. It says nothing about whether omoya
can draw a pixel; that is M2.
