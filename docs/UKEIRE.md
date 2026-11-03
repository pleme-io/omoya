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

> **★ That last row is the state this document was written to describe, and it
> is now history in both halves.** The vocabulary made the seat *read* the
> declaration (below), and the layout registry made it able to *realise* `br`
> (see the RESOLVED section). ggg's seat would now get ABNT2. Kept unedited
> because the drift it records is the thing worth remembering, not the value.

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
| what a touchpad gesture means | **`nazoru`**, configured by `ukeire.touchpad` | ukeire holds the typed knobs; the leaf crate owns the state machine. See "The touchpad" below |
| mouse acceleration | **nobody, and nothing to configure** | a mouse stays raw evdev, so no acceleration layer exists for it. Named so the absence reads as measured rather than as an oversight |
| the `/120` in the scroll math | `wl_pointer.axis_v120` | a protocol unit, not a preference. `v120_multiplier` owns it and does not expose it — exposing it would invite an operator to "fix" scrolling by editing a wire constant |

## The nix seam — one declaration, projected

```nix
services.omoya.settings.ukeire = {
  keymap.layout = config.services.xserver.xkb.layout;   # ← projected, not re-declared
  repeat  = { delay_ms = 200; rate_hz = 45; };
  scroll  = { direction = "traditional"; factor = 3.0; };
  pointer = { cursor_scale = 2; };
  touchpad = { enabled = true; speed = 1.0; scroll_direction = "natural"; };
  modifier = "super";                                    # super | alt
};
```

The layout **defaults from** `services.xserver.xkb.layout`, so the seat and
the TTY cannot disagree unless someone overrides on purpose. That is the
census payoff: the vocabulary subtracts a divergence rather than adding a
knob.

### ★ CORRECTION, measured live 2026-09-03: the seat can only realise `us`

The paragraph above describes the declaration correctly and would leave you
believing the *behaviour* is fixed. It is not, and the difference was found by
running it rather than by reading it.

This workspace patches `libxkbcommon` out for the pure-Rust
`xkbcommon-hairetsu`, whose `new_from_names` is:

```rust
if !matches!(layout, "" | "us") || !variant.is_empty() { return None; }
Some(Keymap::us())
```

So `layout = "br"` compiles to `BadKeymap`. Measured on a live nested seat on
plo:

```
ERROR ukeire: keymap did not compile — keeping the bare-tier keymap
      layout="br" error=BadKeymap
```

while `xkbcli compile-keymap --layout br` **succeeds on the same machine** — so
the xkeyboard-config data is present and reachable, and it is hairetsu that
declines. Two hypotheses were tested and refuted first (a missing
`XKB_CONFIG_ROOT`, and empty `rules`/`model` alongside a named layout); neither
was the cause.

**What the vocabulary still buys, and it is not nothing.** Before it, a non-US
node got a US keymap with *no signal whatsoever* — plo's hardcoded `us` and
declared `us` agreeing by coincidence. Now the same node gets an ERROR line
naming the layout, and `ukeire_keymap_layout` reads `<bare>` rather than
confirming a change that did not happen. That is the difference between an
unsolved problem and an invisible one.

**Why the config is NOT made to refuse a non-`us` layout:** it would make the
truth about ggg's keyboard undeclarable, and the destination is hairetsu
learning rules files, not the config forgetting layouts.

Pinned by `the_xkb_floor_is_us_only_and_this_test_fails_when_that_changes`,
which asserts the limit so it cannot be quietly restated as a fact about xkb —
it is a fact about our own code, and therefore ours to dissolve.

`pending-ukeire-layouts: hairetsu reads xkeyboard-config rules`

### ★★ RESOLVED — the floor moved: `br` is realised, and layout is a REGISTRY

**Everything above this heading is history. Read it for the diagnosis, not for
the current state.** The section is kept rather than rewritten because the
tripwire it describes is the reason the gap got closed instead of becoming
folklore: `the_xkb_floor_is_us_only_…` re-probed the limit on every `cargo
test`, and its failure message named its own follow-up. When the assertion
flipped, this row, that note and the test moved in the same commit — which is
what the message told the next reader to do.

**What changed.** `layout` stopped being a constant with a name and became a
parameter. `hairetsu::layout::LAYOUTS` is a registry of `LayoutDef { rmlvo,
display, keys }`; `Keymap::for_layout(rmlvo)` resolves against it; and
`xkbcommon-hairetsu::new_from_names` — the one function the old paragraph
quotes in full — is now that resolution rather than a literal match on `"us"`.

Adding `br` as a second `if` would have left the class open. As a registry, a
layout is a **row**, and the conformance matrix in `layout.rs` runs every
property against every row — sortedness, unique key names, type↔level-count
agreement, a reachable `ISO_Level3_Shift` wherever four-level columns are
declared, and resolvability. **A row added without satisfying them fails the
build**, so the next layout cannot ship half-transcribed.

**What `br` is.** Transcribed from xkeyboard-config 2.46 `symbols/br` (the
`abnt2` block, which is the `br` default — there is no variant named `abnt2`,
so bare `br` in `nodes/ggg` was always the correct declaration), composed as
`latin(basic)` ⊕ the 21 Brazilian overrides ⊕ `level3(ralt_switch)` ⊕
`kpdl(comma)`. Concretely: **Ç on `<AC10>`** where `us` has `;`, `;` displaced
to `<AB10>`, the extra **`<AB11>`** key `us` does not have at all, the
acute/grave and tilde/circumflex dead-key pairs, `dead_diaeresis` on Shift+6,
`AltGr` bound through `ISO_Level3_Shift`, and a **comma** on the numpad
separator.

**Two things this unblocked that were not the point.**

1. **The four-level machinery was already written and had never had a
   consumer.** `State::level_for_key` has handled `MOD5` + `levels.len() >= 4`
   since the crate was written, and `keysym_to_modifier` already mapped
   `ISO_Level3_Shift`; the `us` table simply had zero four-level keys, so the
   arm was dead code that looked live. `br` is its first consumer — this was a
   wiring job, not a build.
2. **The emitter had no four-level type at all**, so even a four-level table
   would have emitted a keymap in which clients could never reach level 3.
   `FOUR_LEVEL` and `FOUR_LEVEL_ALPHABETIC` now exist in `TYPES`.

**★ And the parity test the emitter's header had been promising found a real
bug on its first run.** That header called the resolver↔emitter agreement "the
seam this module cannot make unrepresentable, and the parity test is what
guards it" — and there was no such test; the ones present are structural
(sections, braces, name spelling) and all pass while the two sides disagree.
The real one parses the emitted `xkb_types` text and compares it against
`level_for_key` for every key of every layout across ten modifier
combinations (~2 200 pairs). It came back red on **Shift+NumLock**, and
settling it against `types/numpad` showed **both sides were wrong**: upstream's
default `KEYPAD` type has `map[NumLock]=Level2` and `map[Shift+NumLock]=Level1`
and **no `map[Shift]` at all**. We emitted a `map[Shift]=Level2` upstream does
not have, and resolved `shift || num` where the rule is `num && !shift`. Both
now match upstream, so Shift on the keypad is the cursor key and shift-select
works with NumLock on.

A disagreement at that seam is invisible in the worst way: we resolve keysyms
for our own bindings, the client resolves them from the emitted text, and a
mismatch means the compositor and the application believe different keys were
pressed with no error on either side.

### ★ Who composes the dead keys — and why the seat does not need `text-input-v3`

ABNT2 is a **dead-key layout**: `ã`, `ç`, `â`, `é` are all two keystrokes, and
`<AD11>` / `<AC11>` produce `dead_acute` / `dead_tilde` rather than characters.
So a reasonable reader asks whether shipping the layout is worth anything while
omoya advertises neither `zwp_text_input_v3` nor `zwp_input_method_v2`.

It is, because **composition is the client's job, not the compositor's.** The
compositor's whole contribution is to deliver the right keysym and hand over a
keymap that names it. What the client does with `dead_acute` is between the
client and its own toolkit.

Traced for our own flagship client, mado (2026-09-09, from source):

| step | where | what happens |
|---|---|---|
| 1 | omoya | emits the `br` keymap text; `<AD11>` level 1 is `dead_acute` |
| 2 | winit 0.30 | compiles that text with the REAL `libxkbcommon`, loaded dynamically (`xkbcommon-dl`) — the client is not on hairetsu |
| 3 | winit | `platform_impl/linux/common/xkb/compose.rs` — `xkb_compose_state_new`, `feed(keysym)`, `xkb_compose_state_get_utf8` |
| 4 | winit → madori | a completed sequence surfaces as `WindowEvent::Ime` (`madori/src/app.rs:1131`) |
| 5 | madori | `set_ime_allowed(true)` is called on window creation (`app.rs:805`), which is what makes winit emit those events at all |
| 6 | mado | `AppEvent::Ime(ImeEvent::Commit(text))` → `engine.on_ime_commit(text)` → the PTY (`gui_tear_attach.rs:1127`) |

Every link is present in code that already ships. **Not verified live** — say
so rather than rounding it up; the honest claim is "the client-side half exists
and is reachable", and the check is to type `ç`, `ã` and `á` into mado on a
`br` seat.

Two consequences worth stating plainly:

- **`text-input-v3` is for input METHODS** (fcitx5/ibus, CJK, a virtual
  keyboard) — a separate capability from dead keys, still absent, still worth
  having. Dead keys are not the argument for it.
- **A client that does NOT compose sees `dead_acute` and produces nothing.**
  That is correct behaviour on every compositor, not an omoya defect — but it
  is the thing to check first if a non-mado client cannot type accents.

**Still refused, deliberately:** any layout not in the registry, and any named
variant. Handing back a base table for a requested variant is the same silent
substitution in a new costume.

`pending-ukeire-variants: hairetsu reads xkeyboard-config rules, so a named
variant (`us(dvorak)`, `br(nodeadkeys)`) resolves rather than refuses`

## The touchpad: `nazoru` (なぞる, to trace with a finger)

Added 2026-11-03 for zek, a Samsung Galaxy Book4 360. `crates/nazoru` is the
leaf crate SEAT-PLAN S-6 asked for: no smithay, no seat, no `/dev`, no C. It is
a pure state machine fed `(type, code, value, time)` and it runs on a Mac.

**Real-hardware behaviour is UNVERIFIED.** No finger has touched this code. The
recordings in `crates/nazoru/tests/recordings/` are written by hand to the
kernel's multitouch protocol and zek's measured axes, in `evemu-record` format,
so a real capture can replace any of them unchanged.

### What the device is, measured on zek

`/proc/bus/input/devices` and `EVIOCGABS` on `event11`, 2026-11-03.

| fact | value |
|---|---|
| name | `ZNT0001:00 14E5:650E Touchpad`, I2C HID |
| `PROP` | `5`: `INPUT_PROP_POINTER` + `INPUT_PROP_BUTTONPAD`, a clickpad |
| `ABS_MT_POSITION_X` | 0..1888 at 16 units/mm, 118 mm |
| `ABS_MT_POSITION_Y` | 0..1296 at 16 units/mm, 81 mm |
| `ABS_MT_SLOT` | 0..4, five slots |
| `ABS_MT_TOOL_TYPE` | 0..2, so the firmware can say `MT_TOOL_PALM` |
| sibling `Mouse` node | `KEY=30000 0 0 0 0`, `REL=3`, expected quiet in multitouch mode |

zek's touchscreen (`GXTP7936`, `PROP=2`, only `BTN_TOUCH`) and stylus (`WCOM0172`)
are neither a touchpad nor a pointer, and a test asserts both from their bitmaps.

### What "stop the touchpad lying" turned out to mean

`open_one` claimed `CAP_POINTER` on `BTN_LEFT || REL_X`, so this pad was already
a pointer that could never move anything. Worse, every `EV_KEY` at or above
`BTN_MISC` became a `PointerButton`, so a touch sent clients a press of
`BTN_TOUCH` and `BTN_TOOL_FINGER`. Narrowing the claim to `REL_X` would have made
the lie smaller; decoding the device makes it true.

`Capabilities::classify` is the one classifier. `pointer` keeps the old rule
unchanged, so no other device moves. `touchpad` is new and needs `BTN_TOOL_FINGER`,
`BTN_TOUCH`, `ABS_MT_SLOT`, both positions and `ABS_MT_TRACKING_ID`, and refuses
`INPUT_PROP_DIRECT`, `REL_X` and `BTN_TOOL_PEN`. A pad the machine cannot decode
keeps exactly what it had. `input_devices` marks a decoded pad `[pt]`.

### Why a mouse is untouched

The backend gives the machine only what `owned_by_pad` claims, `EV_ABS` and
`EV_KEY` at or above `BTN_MISC`, and only on a device classified as a touchpad.
Every other event on every other device takes the old path. A mouse's
`delta_unaccel` equals its `delta`, and `PointerTransfer`'s default is the
identity, bit for bit. **The transfer is not wired to the mouse flush:** that step
stays with S-6's default-flip, which needs a witnessed session.

### The rules the machine holds

| rule | why |
|---|---|
| a new touch has zero delta | a palm or a resting thumb must not jump the pointer |
| X and Y stay sticky per slot, across contacts | the kernel filters a repeated value, so a new touch at the old X sends no X at all |
| a contact excluded last frame has zero delta on return | a palm turning into a finger must not jump |
| one step over 20 mm is discarded and re-anchored | evdev's `SYN_DROPPED` repair re-reads `ABS_MT_*` for the CURRENT slot only, so a position can arrive stale. 20 mm in one frame is not a finger |
| `BTN_TOUCH=0` clears every non-palm contact | the same repair cannot restore a lift lost in another slot, so a ghost finger heals here. It also spoils the tap: a lift nobody saw proves nothing |
| a press decides its button once, and the release repeats it | a click that starts right and ends elsewhere must not release left |
| a tap starts only from zero fingers | a finger left down after a release must not click again |
| a rest-zone contact stops counting past the tap time, or at once if a finger was already down | a resting thumb must not turn pointing into scrolling. A lone short tap there is still a tap |
| a finger left after a scroll does not point until all lift | lifting two fingers one at a time must not nudge the pointer |

### The scroll wire format

The machine measures scroll in the 120-per-detent unit, and one detent is 1.5 mm
of finger travel, so content tracks the fingers at the pointer's 10 px/mm base
rate. What reaches a client is `wl_pointer.axis` at 15 per detent, source
`finger`, then `axis_stop`, with **no `axis_value120`**. smithay's own doc says a
touchpad "may never issue this event as it has no steps", and `input.rs` already
records that winit, GTK4, Qt6 and Chromium read `value120` when it is present and
discard the continuous delta. `axis_sign` exempts `finger` from
`ukeire.scroll.direction`, or a wheel set to natural would invert the touchpad a
second time.

### One trap in the seat's startup

`config::load()` ran AFTER the backend attached input, so `data.state.config` at
that point is still the prescribed tier, and every touchpad key would have parsed,
published and done nothing. The load moved above `match args.backend`, the value
is a parameter down to `EvdevBackend::new`, and
`the_config_is_loaded_before_the_seat_attaches_its_input` reddens if it moves back.

### The keys, all under `ukeire.touchpad`

Numbers clamp at the parse boundary, words and typos refuse. Every constant below
is a guess no finger has judged.

| key | default | range | what to expect from it |
|---|---|---|---|
| `enabled` | `true` | | `false` is the old transport for that device, exactly |
| `speed` | `1.0` | 0.1..5 | times 10 px/mm |
| `acceleration` | `adaptive` | `flat`, `adaptive` | |
| `accel_knee_mm_s` | `50` | 0..500 | no gain below this finger speed |
| `accel_slope` | `0.01` | 0..0.1 | gain per mm/s past the knee |
| `accel_max_gain` | `2.5` | 1..5 | reached at 200 mm/s by default |
| `tap_to_click` | `true` | | |
| `tap_max_ms` | `180` | 50..500 | first contact to last lift |
| `tap_max_travel_mm` | `2.0` | 0.5..10 | also the two-finger slop before scrolling starts |
| `scroll_direction` | `natural` | `natural`, `traditional` | independent of `ukeire.scroll.direction` |
| `scroll_speed` | `1.0` | 0.1..5 | |
| `right_click_width_pct` | `50` | 10..100 | from the right edge |
| `right_click_height_pct` | `20` | 5..50 | from the bottom edge, 16 mm on zek, and also the rest zone |

`nazoru` is `publish = false`: minting a crates.io name is permanent and
outward-facing, so it is the operator's call rather than a side effect of a push,
the same decision as `kasane`.

`pending-nazoru-witness: run a session on zek and record the verdict; tune the
keys above; replace the hand-written recordings with captures`

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
| an uncompilable xkb layout | `set_xkb_config` returns `Err` **without disturbing the live keymap**, so the operator stays on a seat they can log in and fix it from. The fallback is structural, not a branch. Verified live: an unregistered layout → `BadKeymap`, seat keeps its keymap, leaf reads `<bare>`. *(The worked example used to be `br`; `br` now compiles — see the RESOLVED section. `de` is the current unregistered case.)* | parse-time-rejected |
| a layout the seat cannot realise, applied silently | it cannot be silent: an ERROR names the layout and `ukeire_keymap_layout` publishes `<bare>` instead of the requested name. **`br` is now realised** — the residual case is a layout outside `hairetsu::layout::LAYOUTS`, or any named variant | only-mitigated (C2 — observable and logged; the residue is `pending-ukeire-variants`) |
| a `br` request answered with the `us` table | asserted as **difference**, not success: both the `xkbcommon` shim and omoya's own tripwire compile `br` and `us` and require the emitted keymap text to differ. `is_some()` alone would pass on exactly this defect | CI-caught |
| a four-level table whose `AltGr` column is unreachable | the matrix refuses any layout that declares a 4-level key and binds no `ISO_Level3_Shift` — the shape where every level-3 keysym is present and none is typeable | CI-caught |
| a key type disagreeing with its level count | `KeyType::levels_ok` is asserted for every key of every registered layout. A `FOUR_LEVEL` key with two keysyms emits a keymap clients accept and then produces nothing on `AltGr` | CI-caught |
| our resolver and the emitted keymap disagreeing about a modifier | the emit-parity test **parses the emitted `xkb_types` text** and compares it against `level_for_key` for every key × 10 modifier combinations. Found a real Shift+NumLock divergence on its first run | CI-caught |
| a new `KeyType` silently resolving to level 0 | `level_for_key`'s `match` is exhaustive with no `_` arm, so adding a type is `E0004`. This is what forced the four-level arms to land with the four-level types | truly-unrepresentable |
| a remap that rewrites a VT-switch key | `Remaps`'s own `Deserialize` refuses it — the whole `OmoyaConfig` fails to parse and `load()` falls back to the prescribed tier. `Reserved::fleet_linux()` is a pure function of nothing, so the claim set is available *at the parse boundary*; no `Remaps` value in the crate carries one and the only bypass, `unchecked`, is `pub(crate)` and unreachable from any config path | parse-time-rejected |
| a self-remap or a duplicated remap source | same constructor, all problems named in one message | parse-time-rejected |
| the seat's keymap diverging from the node's declared layout | **eval-caught**: the nix module defaults `ukeire.keymap.layout` from `services.xserver.xkb.layout`, so agreement is a projection rather than two hand-lists | only-mitigated (C1 — a *default*, so an operator can still override the two apart on purpose. A `readOnly` derived option would make it truly eval-rejected; that is M1, and deliberate: a dual-layout seat on a single-layout TTY is a legitimate thing to want) |
| the reserved-key table drifting from `awase`'s claims | `reserved_codes` filters the table **through `Reserved`**, so the claim set is the denominator; a claim on a key absent from the table fails `every_reserved_claim_maps_to_an_evdev_code_this_file_knows` | only-mitigated (C2 — CI-caught, fail-closed) |
| a touchpad number out of band | `Refined`'s `Deserialize` clamps; `PointerTransfer`'s fields are `Refined` too, so no curve holds an out-of-band value | truly-unrep |
| a new touch jumping the pointer | a fresh contact's delta is zero and only `advance` writes another; `a_new_touch_starts_with_zero_delta_wherever_it_lands` | CI-caught |
| the touchpad knobs parsed and ignored | a required parameter down to `EvdevBackend::new`, plus a source-order test on `main.rs` | CI-caught |
| a touchpad inverted twice by the wheel's direction | `axis_sign` exempts `finger`; `a_finger_source_is_never_signed_by_the_wheels_direction` | CI-caught |
| `nazoru`'s kernel numbers drifting from the kernel's | the leaf declares them to stay free of evdev, and `nazorus_kernel_codes_are_the_kernels` compares each to the evdev crate on Linux | CI-caught |
| the machine behaving wrong on a real pad | nothing here can see that; no finger has touched it | **unverified** |

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
| `the_xkb_floor_is_us_only_and_this_test_fails_when_that_changes` | *none needed* | It IS the red run — it asserts a limit, so it fails the day the limit lifts, which is the only moment anyone needs telling |
| `a_new_touch_starts_with_zero_delta_wherever_it_lands` | `Contact::settle` writes `delta_mm = at` | RED with `Motion { dx: 62.5, dy: 62.5 }`, a real 62.5 px jump; 14 other tests go red with it |
| `the_config_is_loaded_before_the_seat_attaches_its_input` | `load()` moved below `match args.backend`, the seat handed the default | RED — *"`config::load` at byte 26039 runs after the seat starts at byte 24919"*. Broken in one run with the row below; those two were the only reds of 390 |
| `a_finger_source_is_never_signed_by_the_wheels_direction` | `axis_sign` always returns the wheel's sign | RED — *"Natural gave -1"* |

`nazoru` also ran through `cargo mutants` (25.3.1, 2026-11-03): 304 mutants, 294
caught, 5 unviable, 5 missed. The first pass missed 19 and found eight real test
gaps and two pieces of redundant code. All 5 left are equivalent: negating a whole
axis of the slop leaves `hypot` unchanged (2), dividing by a sign of ±1 equals
multiplying by it (2), and which of two equally fast fingers leads a drag (1).

### Live verification over MCP (2026-09-03)

Run against a `--backend nested` seat on plo — a client window, so the
operator's live seat was never restarted. The nested seat loaded the **real
deployed** `/home/luis/.config/omoya/omoya.yaml`.

| leaf | default config | every-knob-changed config |
|---|---|---|
| `ukeire_repeat_delay_ms` | `200` | `500` |
| `ukeire_repeat_rate_hz` | `45` | `10` |
| `ukeire_scroll_factor_milli` | `3000` | `7500` |
| `ukeire_scroll_natural` | `0` | `1` |
| `ukeire_cursor_scale` | `2` | `4` |
| `ukeire_remaps` | `1` | `1` |
| `ukeire_modifier` | `super` | `alt` |
| `ukeire_keymap_layout` | `us` | `<bare>` (asked for `br`) |

8/8 `found` in both runs, every value as predicted. The `<bare>` in the right
column is the correction above, observed rather than argued.

★ Read against the *old* running seat, the same eight leaves returned
`outcome: refused` with the hint *"the SEAT IS OLDER THAN THIS TOOL — it has
not restarted since the deploy."* Never `blind`, never a fabricated value —
`kotae`'s four outcomes earning their keep.

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
- **Read the tier on each row; the ledger is not uniformly strong.** The three
  `only-mitigated` rows are the *keymap-agrees-with-the-node* row (a nix
  **default**, deliberately overridable), the *table-tracks-awase's-claims* row
  (C2, CI-caught), and the *layout-cannot-be-realised* row (C2 — observable, not
  fixed). The touchpad's behaviour rows are CI-caught against hand-written
  recordings, and its last row is unverified.
- **The keymap knob is a DECLARATION, not yet a capability.** Only `us` and
  the empty default actually apply. See the correction above; do not cite this
  vocabulary as having fixed non-US keyboards.
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
