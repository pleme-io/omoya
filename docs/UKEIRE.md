# ukeire (受け入れ) — the seat's intake of physical input

> **The one question:** when a physical input event arrives, what does it MEAN
> and how fast do we take it?
>
> Everything omoya decides between the kernel handing it an evdev event and a
> client receiving a Wayland one. One typed value, one place.

Authored 2026-09-03. Name checked against the corpus rather than reasoned
clear: `uketsuke` (受付, the reception desk) was already **mukae**'s, for the
reception of a *person*, so input intake took its own word rather than sharing
a question with the login manager.

## Why it exists

Every answer in this domain was a literal, and the scattering was not a
tidiness complaint. Censused 2026-09-03:

| fact | where it lived | what it was |
|---|---|---|
| keymap | `state.rs`, `XkbConfig::default()` | **hardcoded US, no options** |
| repeat delay/rate | `state.rs`, an argument pair | `600`/`25` |
| scroll magnitude | `input.rs`, mid-expression | `* 3.0 / 120.` |
| scroll direction | nowhere | **not representable** |
| seat modifier | `deed.rs`, `pub const LOGO` | `Modifiers::CMD` |
| cursor scale | `cursor.rs`, `pub const SCALE` | `2` |
| cursor dimensions | `cursor.rs` | `10` and `17`, restating `ART` |
| remaps | `config.rs` | already configured — the one done right |

### The finding that justified the vocabulary

The keymap row is not merely unconfigured, it is **silently divergent**. Three
declarations of "what layout is this machine" already existed in the fleet's
nix tree:

| declaration | scope | plo | ggg |
|---|---|---|---|
| `services.xserver.xkb.layout` | X + the fleet's convention | `us` | `br` |
| `blackmatter.profiles.blizzard.console.keyMap` | text TTYs | `us` (default) | `br-abnt2` |
| `org/gnome/desktop/input-sources` | GNOME session | derived from the first | derived |
| **omoya's Wayland seat** | **the actual desktop** | **`us`, hardcoded** | **`us`, hardcoded** |

omoya read **none** of them. Its hardcoded US agrees with plo **by
coincidence** — which is exactly why nobody noticed — and would hand gabi a US
keymap on a Brazilian ABNT2 keyboard the day omoya reaches ggg. The drift is
**latent, not live**: `ggg` runs GNOME, and omoya is enabled only on `plo`
(measured — `grep -rl omoya nodes/`).

Latent drift that reads correct at every layer is the shape the fleet's
dated-claim rule warns about, and it is worse than a wrong value because
nothing will ever flag it.

**So `Keymap`'s job is not to be a fourth declaration.** It is the
*projection target* of the one that already exists — see "The nix seam" below.

## What it deliberately does not own

Naming these keeps the weave flat: no two layers answer one question.

| question | owner | why not ukeire |
|---|---|---|
| which chord does what | `deed.rs` over `awase` | a binding question, not an intake one |
| what may not be bound | `awase::Reserved` | ukeire *consults* it; it never re-states a claim |
| bounding a runaway held key | `awase::KeyRepeatGate` | that is at the **consumer**; ukeire sets the seat's advertised pace. Keeping them apart is what lets the seat be fast while a password field stays safe |
| pointer accel / tap-to-click | **nobody, and nothing to configure** | omoya reads raw evdev, not libinput, so no acceleration layer exists. Named so the absence reads as measured rather than as an oversight |
| the `/120` in the scroll math | `wl_pointer.axis_v120` | a protocol unit, not a preference. `v120_multiplier` owns it and does not expose it — exposing it would invite an operator to "fix" scrolling by editing a wire constant |

## The nix seam — one declaration, projected

```nix
services.omoya.settings.ukeire = {
  keymap.layout = config.services.xserver.xkb.layout;   # ← projected, not re-declared
  repeat  = { delay_ms = 200; rate_hz = 45; };
  scroll  = { direction = "traditional"; factor = 3.0; };
  pointer = { cursor_scale = 2; };
  modifier = "super";                                    # super | alt
};
```

The layout **defaults from** `services.xserver.xkb.layout`, so the seat and
the TTY cannot disagree unless someone overrides on purpose. That is the
census payoff: the vocabulary subtracts a divergence rather than adding a
knob.

## Tier ledger

Every bad state the vocabulary was built to corner, at its **true** tier. A
`Result::Err` is mitigation; a compile error is unrepresentability. Nothing
here is rounded up.

<!-- tier-ledger -->

| bad state | how the vocabulary corners it | tier |
|---|---|---|
| repeat delay or rate out of band | `Refined<i32, B>`'s `Deserialize` clamps at the parse boundary; the struct has no field that can hold an out-of-band value | truly-unrep |
| repeat disabled *with* a delay | `rate_hz = 0` is Wayland's own spelling for off, so there is no `enable` bool and the combination has no representation | truly-unrep |
| scroll inverted by a stray minus | direction is a closed `ScrollDirection`, magnitude a separate bounded `f64`; a negative factor clamps and cannot reach `sign()` | truly-unrep |
| dead scroll (factor 0) | the lower bound is `0.25`; "no scrolling" is not a small magnitude and has no spelling | truly-unrep |
| seat modifier set to `ctrl` (soft-bricks the box — every fleet chord would collide with `Ctrl+Alt+F1..F12`, removing the VT escape) | `SeatModifier` is a two-variant closed enum; `ctrl` does not deserialize and no expression constructs it | truly-unrep |
| cursor dimensions disagreeing with the art | `CELLS_W`/`CELLS_H` are `ART[0].len()` / `ART.len()`; the mask is the single source | truly-unrep |
| a typo'd knob absorbed silently | `deny_unknown_fields` at every level — the loader `Err`s | parse-time-rejected |
| an uncompilable xkb layout | `set_xkb_config` compiles through libxkbcommon and returns `Err` **without disturbing the live keymap**, so the operator stays on a seat they can log in and fix it from. The fallback is structural, not a branch | parse-time-rejected |
| a remap that rewrites a VT-switch key | `Remaps`'s own `Deserialize` refuses it — the whole `OmoyaConfig` fails to parse and `load()` falls back to the prescribed tier. `Reserved::fleet_linux()` is a pure function of nothing, so the claim set is available *at the parse boundary*; no `Remaps` value in the crate carries one and the only bypass, `unchecked`, is `pub(crate)` and unreachable from any config path | parse-time-rejected |
| a self-remap or a duplicated remap source | same constructor, all problems named in one message | parse-time-rejected |
| the seat's keymap diverging from the node's declared layout | **eval-caught**: the nix module defaults `ukeire.keymap.layout` from `services.xserver.xkb.layout`, so agreement is a projection rather than two hand-lists | only-mitigated (C1 — a *default*, so an operator can still override the two apart on purpose. A `readOnly` derived option would make it truly eval-rejected; that is M1, and deliberate: a dual-layout seat on a single-layout TTY is a legitimate thing to want) |
| the reserved-key table drifting from `awase`'s claims | `reserved_codes` filters the table **through `Reserved`**, so the claim set is the denominator; a claim on a key absent from the table fails `every_reserved_claim_maps_to_an_evdev_code_this_file_knows` | only-mitigated (C2 — CI-caught, fail-closed) |

### Red runs

House standard is a recorded red run per seal. All four, on rio:

| gate | break | result |
|---|---|---|
| `the_default_repeat_is_fast_enough_to_be_worth_having` | restore 600/25 | RED — *"the first repeat must arrive within 300ms, got 600ms"* |
| `natural_scrolling_inverts_the_sign_and_nothing_else` | `Natural => 1.0` | RED |
| `remapping_a_vt_switch_key_is_refused` | delete the reserved check | RED |
| `an_unconfigured_keymap_is_exactly_the_old_hardcoded_behaviour` | `Default` prescribes `us` | RED — **and it also took down `config::every_tier_hands_the_seat_a_usable_intake_policy`**, which is the tier gate noticing the same defect from the other side |
| `a_soft_bricking_remap_is_refused_by_the_config_loader_itself` | `Deserialize` returns `Ok(unchecked(...))` instead of refusing | RED |
| `every_reserved_claim_maps_to_an_evdev_code_this_file_knows` | *none needed* | RED **for free**: it failed with `derived 0` while the matcher was broken, and passed once fixed |

★ That last row is the useful one. The first draft of `reserved_codes` had
**two** independent bugs — a `contains` substring match (so `"ctrl+alt+f12"`
reports F1 protected because F12 is) and a case mismatch against a lowercase
canonical spelling. The denominator test caught them before the feature
shipped, which is what a denominator-in-the-assertion is for. The matcher now
compares the **last `+`-segment exactly**, so the substring class is
unrepresentable rather than carefully avoided.

### The C1 rows were lifted, not merely named

The first cut of this ledger graded the two remap rows `only-mitigated (C1)`
with the ceiling stated: *"a runtime check, not a type — a caller who forgot
to consult `refusals` would apply a soft-bricking remap with no complaint."*
That ceiling is now gone, and the move that removed it is worth recording
because it looked unavailable:

**`awase::Reserved::fleet_linux()` is a pure function of nothing.** It takes no
config, no environment, no caller context. So `Deserialize` — which has no way
to receive context — can construct the claim set *itself*. The refusal moved
from a method someone calls to the boundary every config value must cross.
`Ukeire::refusals` survives as a delegating diagnostic ("why would this be
rejected?") rather than as the guard, so the two cannot disagree.

★ The bounded leaves **clamp** and this one **refuses**, and the asymmetry is
the difference between a preference and a hazard. An out-of-band repeat rate
has an obviously-right nearest legal value. A remap that eats the VT escape has
none: dropping it silently would leave the operator believing CapsLock was
remapped when it was not, and keeping it would cost them the machine.

## Tier honesty — what this is NOT

- **M0 is plain typed Rust plus shikumi config.** The `(defukeire …)`
  tatara-lisp form and its `#[derive(DeriveTataraDomain)]` border are a
  **named M1**. `specs/ukeire.lisp` *documents* the destination form; it is
  not the wired form and nothing loads it.
- **Nine of eleven rows are now truly-unrep or parse-time-rejected.** The two
  that remain `only-mitigated` are the *keymap-agrees-with-the-node* row (a
  nix **default**, deliberately overridable) and the *table-tracks-awase's-claims*
  row (C2, CI-caught).
- **Observation is not proof.** The eight `ukeire_*` introspect leaves publish
  the policy the seat RESOLVED, which is a measurement, never a guarantee that
  it is the right policy.
- **Nothing here makes input *correct*.** It makes it *declared*. Whether
  `br` is the right layout for a given keyboard is a fact about the world, and
  the world's answer lives in the node's nix config — which is precisely why
  the seat projects from it instead of holding an opinion.

## Observing it — the eight leaves

`ukeire` publishes the policy the seat **resolved**, over kanshou/MCP. The
distinction from the requested policy is the whole reason these exist: a seat
whose keymap failed to compile keeps a working keymap and otherwise looks
identical to one that applied the operator's, because both come up and both
type.

| leaf | note |
|---|---|
| `ukeire_repeat_delay_ms` | |
| `ukeire_repeat_rate_hz` | `0` means repeat is off |
| `ukeire_scroll_factor_milli` | x1000, so `3.0` does not arrive as `3` |
| `ukeire_scroll_natural` | `1`/`0` — deliberately NOT folded into the factor's sign, which is the conflation `ScrollDirection` exists to stop |
| `ukeire_cursor_scale` | |
| `ukeire_remaps` | count |
| `ukeire_modifier` | `super` / `alt` |
| `ukeire_keymap_layout` | **`<bare>` when the requested layout did not compile** — published only on a successful apply, so the observation plane never confirms a change that did not happen |

## M1, named rather than implied

1. `(defukeire …)` + the TataraDomain border.
2. Live re-application (`calha`-style) so a config change reaches a running
   seat without a logout. Today all of it applies at startup, which is also
   why a stale config cannot poison a running seat: `config::load()` is called
   exactly once and there is no watcher.
3. Lifting the keymap-agreement row from a nix `default` to a `readOnly`
   derived option — deliberately NOT done: a dual-layout seat on a
   single-layout TTY is a legitimate thing to want.

### Done since the first cut

- ~~`Remaps` as a newtype on `Ukeire`~~ — landed; the yaml key moved to
  `ukeire.remaps` (measured safe: no nix file and no live seat declared a
  top-level `remaps`).
- ~~A reserved-excluding constructor to lift the two C1 rows~~ — landed as
  `Remaps::parse`, refusing at the `Deserialize` boundary.
