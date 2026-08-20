<!-- Generated 2026-08-20 by a measured, adversarially-reviewed analysis of the seat.
     Every path-bearing claim was read from source, not inferred. Re-measure before
     acting on any colour or count claim — this document is a dated snapshot. -->

Verified on this host, 2026-08-20, before writing. Every claim below that carries a path was read, not inferred.

---

# THE PLEME-IO SEAT — destination-first plan (rev. 2)

## 0. WHAT PLO IS, BEFORE ANYTHING TOUCHES IT

The draft called plo "a live desktop". That is a three-way understatement and one overstatement. Measured:

| Role | State | Source |
|---|---|---|
| k3s control plane | **OFF** since 2026-08-18 | `nodes/plo/configuration.nix:318` `pleme.nixos.k3s.enable = false` |
| FluxCD | **OFF**, followed k3s down | `nodes/plo/configuration.nix:459-462` |
| **LAN DNS server** | **LIVE** | `nodes/plo/configuration.nix:240` dnsmasq listen addresses |
| **Tailnet's only advertised route into 192.168.50.0/24** | **LIVE** | `nodes/plo/tailscale.nix:11` |
| Local console | **SINGLE POINT OF FAILURE** | i7-11700**F** — no iGPU; `boot_vga = 1` on the RTX 3070. A failed DRM handoff leaves **no console at all** (`configuration.nix:320-365`) |
| Recovery path | **ssh only**, plus a 10s boot menu for a human physically present | same |

So the blast radius of a wedged seat is **DNS for the LAN and the tailnet route into the home subnet** — not the cluster. And the recursion is real: the route you would use to reach plo is advertised by plo.

**Consequence, adopted as a standing rule:** no seat change lands on plo until it has (a) passed on vkms and (b) a verified ssh path exists at the moment of landing.

---

## 1. THE DESTINATION

plo boots into a seat where every layer from the scancode to the scanned-out pixel is pleme-io substance and the kernel is the only foreign code beneath it. mukae opens the session itself — credential verified against `/etc/shadow` in pure Rust, session registered with logind over the wire we speak and own, limits set, privilege dropped, `execve` — and hands omoya a set of devices. omoya composites driven by the display's own page-flip *completion*, repaints only what changed, promotes what it can to hardware planes, and receives a client's GPU buffer without the CPU touching a pixel. The keyboard is a generated layout corpus behind `(deflayout …)`, byte-differentialled against real `xkbcli`, keyed on **keysyms** so a `br` layout's AltGr and a `us` layout's right Alt are the same rule rather than two. Input policy is a declared, continuously-reconciled value observable over kanshou. Every device identity, layout and policy is a typed field in `fleet.nix`, resolved once and projected everywhere.

**And on the way there, at every single step, the seat is a place a person can do work in:** a terminal opens, a pointer is visible, a second window is reachable, and walking away locks the screen.

The draft's destination was correct and its ordering was not. A compositor that paints Nord0 with no cursor, no launcher and no window management is not a seat at any frame rate.

---

## 2. THREE STREAMS, NOT ONE PLAN

Critique A is right that this was three projects under one title. They have different entry conditions and different blockers, so they get different tracks. Run **A** and **S** concurrently; **M** does not start until S-4 lands.

| Stream | What | Blocked on |
|---|---|---|
| **A — supply chain** | red CI, `--locked`, lock-vs-plo build gate | nothing |
| **S — the seat** | usable → recoverable → correct → fast | reaching plo |
| **M — the session** | mukae replaces greetd/PAM | a `/etc/shadow` privilege decision nobody has made |

---

## 3. THE DELTA, RANKED BY LEVERAGE

Cost is author-effort. Rows marked ▲ are new since the draft; rows marked ✗ were removed or demoted.

| # | Item | Why it matters | Cost |
|---|---|---|---|
| **1** ▲ | **Pin plo's gitops source off `main`** | One field: `pleme.gitops.source.branch` is a typed record field (`modules/pleme/shared/gitops.nix:61`). Today every push to main reaches the machine whose only console is under change. This is the single highest-leverage reliability change in the document and it costs one line. | **S** |
| **2** ▲ | **The seat can launch a terminal** | `modules/pleme/nixos/seat-render.nix:78` renders `sessionCommand = "${lib.getExe pkg}"` — bare exe. omoya's only spawn path is trailing argv after `--` (`main.rs:48-49, 630`). Log in → Nord0 → the only exit is a VT switch that may not work. `sessionCommand` is `readOnly` and derived from `roles.compositor`; it must derive from `roles.terminal` too. | **S** |
| **3** ▲ | **Draw a cursor; add a keyboard focus path** | `handlers.rs:228-232` — `cursor_image` discards the client's request; nothing draws a pointer. `input.rs:187-196` sets keyboard focus **only** on `PointerButton`. So focus is reachable only by aiming an invisible pointer. Hard prerequisite of every input item. | **M** |
| **4** | **omoya + mukae `main` are RED** | The fleet pins revs from two repos whose gates fail. mukae is red on **both** `Test gate` and `build` — a CI-shape problem (`macos-15`), not one type error. Re-costed. | **S–M** |
| **5** | **`cargo metadata --locked` at push time** | The exact predicate crate2nix uses. One missing lock line cost plo ≈6 h unable to converge (`nix@5c9ab720`). No such check exists in the fleet. Unchanged from the draft and still right. | **S** |
| **6** ▲ | **A `flake.lock` → `plo.toplevel` build gate** (replaces the greenness gate) | The predicate we actually want is "this lock converges plo", and only a build answers it. Scoped to one node, triggered on `flake.lock` only. See §5 for why the draft's gate was deleted. | **M** |
| **7** | **A verified escape hatch on plo** | **Three independent defects, three witnesses** — not one root cause. (a) omoya's `SessionEvent` handler is inert; (b) the RAlt modmap; (c) logind's `TakeControl` puts the VT in `K_OFF`. Fixing any one alone gives no hatch, and the draft's "one root cause" framing is exactly what licensed a one-file fix with a passing test. | **S** probe / **M** fix |
| **8** | **Right Alt: fix it on the KEYSYM, not the key name** | Corrected — see §4. Severity raised: with RAlt off Mod1 and AltGr unverified, a br/ABNT2 keyboard **cannot type `@ \ | ~ { }`**. Not a hotkey nicety. | **S** |
| **9** ▲ | **Suspend/resume round-trip** | Rides the exact `pause()`/`activate()` path as #7 and happens far more often than a VT switch. Free to add to #7's witness; expensive to discover later. | **S** on top of #7 |
| **10** ▲ | **Two windows are usable** | `handlers.rs:109` maps every toplevel at `(0,0)`; `send_configure()` sends no size; `move_request`/`resize_request` are accepted-and-ignored (`handlers.rs:132-141`). Second app = fully overlapped at the origin with no move, resize, alt-tab or close. Also: damage tracking designed against one window hides the real cost model. | **M** |
| **11** ▲ | **Lock / idle blank** | `roles.lock = null` (`profiles/nixos-pleme-omoya/default.nix:216`). The parked Hyprland seat had hyprlock. A machine holding sops-decrypted material that never blanks is a regression against what it replaced. Needs a decision, not a mention. | **M** |
| **12** | **kanshou introspect as test oracle AND perf oracle** | Serves three consumers (scanout verification, dmabuf A/B, vkms non-vacuity). Build once. **Re-costed** — `capture()` at `drm.rs:541` is not partial credit; it has zero call sites and needs `ExportMem` on the renderer (`nuri_renderer.rs:603` says so). | **M–L** |
| **13** | **vblank pacing + swap-on-completion** | `scanout.rs:198-200` swaps `back` when the flip is *accepted*, not when it retires — honestly commented, and still wrong. The timer's real period is `interval + render_time`, so the seat structurally cannot reach panel refresh. | **M** |
| **14** | **Damage tracking** | **Re-costed S→L.** `OutputDamageTracker` is built in `drm.rs:273 pub fn prepare(…)`, which has **zero callers** — the live render path is elsewhere. Adopting it means replacing the loop that superseded it, not "adding damage". | **L** |
| **15** | **dmabuf — measured, not assumed** | `import_dmabuf` (`nuri_renderer.rs:467-475`) *refuses*. Adding `DmabufState` alone routes every client through smithay's blanket `ImportAll` into an `Err` per frame: Nord0, no windows, no DRM error. One unknown decides the design: can nuri `mmap` an nvidia-exported linear dmabuf? | **S** probe / **L** land |
| **16** | **A pointer transfer function** | Raw evdev counts land unmodified; a 1600-CPI mouse crosses plo in ~0.6 in of travel. One scalar multiply. `delta_unaccel` is an identity today, so the raw/accelerated distinction clients ask for does not exist. **Entry corrected: blocked on #3 (a cursor), not on Phase 3.** | **S–M** |
| **17** | **Device identity into `pleme.nixos.gpu.scanoutPath`** | **Re-costed M→S on the nix side:** `scanoutPath` already exists (`modules/pleme/nixos/gpu.nix:367`, computed `:411`). The entire cost is the omoya CLI, which today has **no** `--drm-device`. | **S** nix / **M** omoya |
| **18** | **Input hotplug (netlink udev group)** | A plugged-in keyboard is invisible until restart. `input.rs:277` is a bare `_ => {}`. Ordering decides the design: devtmpfs creates the node before udevd tags it, so inotify and the kernel netlink group both race `TakeDevice`. | **L** |
| **19** ▲ | **Output hotplug** | `probe()` (`drm.rs:179`) takes the first connected connector **once**. `scanout.rs:189-192` correctly claims a *mode* change on the same connector re-modesets via `commit_pending` — but nothing re-runs connector selection, so **plugging in a monitor does nothing**. For a workstation this is more frequent than input hotplug. | **M** |
| **20** | **Layout plumbing + a NixOS assertion** | `services.xserver.xkb.layout = "br"` on ggg reaches nothing. Flipping ggg today gives an ABNT2 keyboard a silent `us` — and per #8, no AltGr. | **M** |
| **21** | **mukae opens the session** | greetd links **linux-pam**, and `pam_systemd` registers with logind anyway. **Re-costed:** this is not near-done. `verify_user` (`mukae-native/src/lib.rs:62`) has zero callers; `create_session` (`logind.rs:221`) has only test callers; `mukae-greeter/Cargo.toml` has **no `mukae-native` dependency at all**. Blocked on an undecided `/etc/shadow` question. | **L** |
| **22** | **`DirectSession`** | Closes `C6`; only path to a seat on a non-systemd host. **Does not remove logind from plo** and makes the compositor root. | **L**, high risk |
| **23** | **Generated corpus; planes; explicit sync** | Destination work. Overlay planes are the only change that can turn a working seat black. | **XL** |
| ✗ | ~~fleet-wide input-greenness gate~~ | **Deleted.** See §5. | — |
| ✗ | ~~"#4 is one root cause wearing three costumes"~~ | **Deleted.** Three defects, three witnesses (now #7). | — |
| ✗ | ~~"the header rounds up to tearing impossible"~~ | **Deleted — the phrase does not exist.** `grep -rn tearing` across omoya returns exactly one hit, `scanout.rs:61`, which does not make that claim. The header's actual admission is "full repaint every frame — the damage rectangles are computed and then ignored", with `pending-omoya-damage` attached. Both the draft and critique A were correcting prose that isn't there. | — |

**Convergent evidence, extraction still owed:** omoya's `logind.rs:64-68` and mukae-native's `logind.rs:208-210` independently define the same three D-Bus constants and pin an identical `zbus 5 { default-features = false, features = ["blocking-api","tokio"] }`; omoya's `Cargo.toml:52` says "logind costs no `.so` — which is the entire reason this replaces libseat". Two crates, one wire, no shared ancestor. A `logind` leaf crate is owed, owned by neither — extracted in M-1, when the second consumer goes live.

---

## 4. THE RALT FIX, CORRECTED

The draft prescribed `emit.rs`: `"LALT" | "RALT" => "Mod1"`. That is wrong twice, and critique A is right on both counts.

**There is no RAlt drift to fix.** Measured:

```
hairetsu/src/lib.rs:221    key::Alt_R | key::ISO_Level3_Shift => modifier::MOD5,
hairetsu/src/emit.rs:63    "RALT" => "Mod5",
```

They **agree**. Editing `emit.rs` alone leaves `State` putting `Alt_R` in MOD5, `ModifiersState.alt` false, and the chord matching nothing — while the draft's done-predicate ("a hairetsu unit test asserting RALT sets Mod1") passes. That predicate was satisfiable with the hatch still dead.

**And a name-keyed `"RALT" => "Mod1"` breaks every AltGr layout.** `layout.rs:157` gives RALT the keysym `Alt_R` in `us`; a br/ABNT2 or us-intl layout gives the same key `ISO_Level3_Shift`, and `lib.rs:426` reads MOD5 for level 3. Hardcoding by key *name* pins RAlt to Mod1 regardless of keysym — so item #8 would be undone by item #20 and made structurally impossible by #23.

**The fix, once, keyed on the keysym:**

- `lib.rs modifier_bit`: split the arm — `Alt_L | Alt_R | Meta_L | Meta_R => MOD1`, keeping `ISO_Level3_Shift => MOD5`.
- `emit.rs`: derive the modmap from the key's **keysym** via the same function, not from `entry.name`. This deletes the seam rather than re-aligning it.

**The parity test must guard the modmap seam, not the level seam.** The drift that actually exists is Scroll Lock: `emit.rs:57` `"SCLK" => "Mod3"` plus `emit.rs:107` `LockMods(Mod3)`, against `lib.rs:227-234` `lock_bit`, which has **only** `Caps_Lock → LOCK` and `Num_Lock → MOD2` — no `Scroll_Lock` arm. A test that feeds masks in directly (what `emit.rs:68` promises) never exercises `modmap_entry` ↔ `modifier_bit`/`lock_bit` and would go green with that divergence intact. The test is: **for every `KeyEntry`, `modmap_entry(entry)` agrees with `modifier_bit(sym)` / `lock_bit(sym)`.** Then drop the `Mod3 { <SCLK> }` modmap and the `Scroll_Lock` interpret, since nothing on the Rust side locks it.

---

## 5. THE GREENNESS GATE IS DELETED. HERE IS WHAT REPLACES IT.

The draft called this the flagship. Critique A killed it on four counts, three of which I re-measured:

| Claim | Draft | Measured, this host, `nix/flake.lock` |
|---|---|---|
| pleme-io github inputs | "~330" | **165 nodes / 154 unique repos** (393 github nodes total — that is where 330 came from) |
| Sampled revs green | implied | **17 zero check-runs · 5 red · 3 green of 25** |

A gate refusing "any non-skipped run not `success`, or zero check-runs" refuses ~22/25 on day one. It ships with ~135 waivers — at which point waived is the default and it detects nothing — or it is deleted in a week. That is the vacuous-gate shape reached from the opposite direction.

Three more, all correct and none defended:

- **Green ≠ buildable.** The red sample is dominated by `release / bump` and `Publish each workspace member` — post-merge publish jobs, not build gates. And this fleet already recorded the counter-example: *a flake's `follows` swaps its tested dependency* — green standalone, broken in the consumer.
- **Wrong surface.** `.github/workflows/checks.yml:138-155` triggers on **push to main**; plo reconciles from main. Both start at the same push. A check-run three minutes later cannot un-deploy a generation that switched at two. This repo's own enforcing surface is `fleet.shipping`/`parts/fleet.nix`, which throws before a byte is built.
- **New fleet-wide PAT on the push path.** Reading check-runs on 154 other repos needs a cross-repo PAT, and `checks.yml:166` resolves to `camelot-builder-pleme-eks` — so the gate would inherit camelot uptime for every push to the private config repo. Memory carries *ONE PAT wedges CI* and *stale-token cascade — one PAT, six surfaces*.
- **And it can wedge the fleet** (critique B): one input red for an unrelated reason blocks `nix flake update` everywhere, in a repo that treats a stale lock as a cardinal sin.

**Replacement, two pieces:**

1. **`nix run .#input-health` — advisory, never a gate.** Prints per-input check state so a bump is an informed decision. Runs from the operator's session with the token that already exists. No push-path dependency, no new PAT, no fleet wedge. Per-**input** `skip:` entries, not per-run.
2. **The real gate, narrowly scoped: build `nixosConfigurations.plo.config.system.build.toplevel` when `flake.lock` changes.** One node, one path filter, on a linux builder (rio, or `checks.yml`'s resolved runner). This is the predicate that answers "can this lock converge plo", and only a build answers it. The draft deferred this claiming greenness bought "most of its value at ~1/1000 the cost" — falsified at 3/25. It costs a build; it is the only thing that is true.

---

## 6. PHASES

### A — Supply chain (repo-only; touches no profile plo imports; runs immediately, in parallel)

**Entry:** none.

1. Fix omoya `release / Test gate` (E0599 `FormatSet::is_empty`) and mukae `Test gate` + `build` — the latter is a CI-shape problem on `macos-15`, budget accordingly.
2. `cargo metadata --locked --format-version 1 >/dev/null` as `pleme-io/actions/cargo-lock-fresh`; adopt in omoya + mukae + mado.
3. Reorder `cargo fmt` after `cargo test` in `substrate/.github/workflows/cargo-ci.yml` — a fmt nit currently masks a compile error.
4. Adopt `reusable-gen-spec.yml` in omoya + mukae; land a `Cargo.gen.lock` in mukae so the pre-commit D2 gate stops being structurally inert.
5. `nix run .#input-health` (advisory) + the `flake.lock` → `plo.toplevel` build gate (§5).

**Done-predicate:**
- `gh api repos/pleme-io/{omoya,mukae}/commits/main/check-runs` → every non-skipped run `success`.
- The `--locked` gate **red-runs** against a deliberately-desynced `Cargo.toml` and goes green after regen.
- The plo build gate **red-runs** against a lock pinned to a rev that does not build, naming the derivation.

**Explicitly NOT in this phase** (the draft put them here and mislabelled the phase "no plo dependency"): every edit to `profiles/nixos-pleme-omoya/default.nix` and the `services.seatd.enable` flip. That profile is imported by `nodes/plo/default.nix:64`. Those move to S-3.

---

### S-0 — Ground truth, and cut the leash (read-only, then one field)

**Entry:** a working path to plo.

**Work:** re-establish ssh (tailscale peer shows `active` via relay `sao`; ssh times out — diagnose which). Then one probe pass, recorded:

- `ls /dev/dri/by-path/`; `drm_info` → atomic or legacy; `cat /proc/bus/input/devices`; `cat /etc/pleme/seat.json`
- **switch to VT2 and back** — the load-bearing question
- **suspend and resume** — same code path, higher frequency
- with a Wayland client focused, check whether keystrokes also reach tty1's line discipline (no `EVIOCGRAB` anywhere in omoya)
- ctrl-C/ctrl-V between two Wayland clients — measured yes/no. (`DataDeviceState` is delegated and wired at `state.rs:105,133` + `handlers.rs:251`, so it plausibly works; nobody has checked. `wp_primary_selection` is absent — middle-click paste is dead.)
- **which gitops backend is actually RUNNING and its last receipt.** `configuration.nix:637` *declares* `sentinela`, and `:625-628` says the handover is out-of-band and needs an operator ssh session — which has been failing. Declared ≠ running.
- **do DNS and the subnet route survive independently of the graphical target?** They should — neither is in it — but that is a measured row, not an assumption.
- **does greetd re-present when the compositor exits?** `restartIfChanged = false` is documented for the deploy path; the crash path is not.

**Then, immediately:** set `pleme.gitops.source.branch = "seat-staging"` on plo and rebuild it. One field (`modules/pleme/shared/gitops.nix:61`).

**Done-predicate:** a committed probe record in `nix/docs/` naming, with values: ssh exit code; the by-path node string; `atomic`/`legacy`; device count; the running gitops backend + last receipt; **and literal yes/no for VT round-trip, suspend/resume round-trip, clipboard, DNS-survives, route-survives, greetd-re-presents**. Plus: plo's next generation reports `seat-staging` as its source.

**What the pin changes about the house rules — stated plainly.** `main` must still build for every other node; that rule is unchanged. plo follows `seat-staging`, which is rebased on `main` regularly and **must build on every commit** — the constraint moves, it does not relax. Un-pin condition, named now so it is not forgotten: **S-4 green plus one full week of `seat-staging` == `main`.**

A "no" on VT round-trip promotes S-2 above everything.

---

### S-1 — The seat is a place you can do work (no plo dependency beyond the pin)

**Entry:** S-0's pin landed.

1. **Launcher.** `seat-render.nix:78` derives `sessionCommand` from `roles.compositor` alone. Derive it from `roles.terminal` too: `"${getExe compositorPkg} -- ${getExe terminalPkg}"`. It stays `readOnly` and stays a projection of two typed role fields — the repo's own derived-option shape, not a hand-written string. Add one spawn chord in omoya (`chord.rs:62 key_from` maps only F1–F12/Delete/Backspace today, solely to feed awase's reserved-chord claims).
2. **Cursor.** Implement `cursor_image` (`handlers.rs:228`) and draw the pointer. This is the prerequisite the draft filed *last*.
3. **Keyboard focus path.** `input.rs:187-196` focuses only on `PointerButton`. Add focus-follows-map for the first window at minimum, so a seat with one client is usable before there is a pointer to aim.

**Done-predicate:** on vkms, then witnessed on plo — log in, a terminal is present without touching a mouse; a visible pointer moves; clicking a second window focuses it. Recorded in the probe file.

---

### S-2 — The escape hatch is real (three defects, three witnesses)

**Entry:** S-0 recorded; A green.

- **(a) hairetsu, keysym-keyed** — the §4 fix, plus the modmap-seam parity test, plus dropping the Scroll Lock divergence.
- **(b) `SessionEvent` actually reacts** — hold the `DrmDevice` + evdev backend where the session closure reaches them; `PauseSession → device.pause()`, `ActivateSession → device.activate(true)`.
- **(c) logind's `K_OFF`** — probe first, then decide. Do **not** "complete" the chord path by swallowing `Ctrl+Alt+Fn` before `change_vt` is genuinely called; today's forward-and-count is an honest no-op and a half-finished grab is a soft brick.
- Wire the `xkbcli` differential as a nix check: `xkbcli compile-keymap --keymap <ours>` exit 0, plus block-level comparison of `modifier_map` and `key <…>` against `--layout us`. Both binaries are packaged; the oracle already accepts our output.

**Done-predicate — three witnesses, no merging:**
1. A hairetsu unit test on the **modmap seam**, red-run against pre-fix code (the level-mask test the draft specified would have passed while the hatch stayed dead).
2. The `xkbcli` differential is a `checks` entry, red-run against the pre-fix keymap, **built by CI** rather than declared.
3. On plo, witnessed **twice**: VT away → back, **and suspend → resume**. In both, the display returns, the keyboard works, and introspect shows pause/activate having been *called*, not counted.

---

### S-3 — The sensor, a gate a black screen cannot satisfy, and the profile cleanup

**Entry:** S-1.

**Work:** extend `OmoyaIntrospect` with `last_flip_seq`, `flip_interval_us`, `missed_flips`, `pixels_painted`, `input_devices` (count + names). Rewrite `checks.vkms-seat` to read the kanshou sidecar; **delete both negative assertions** (`"no logind session" not in journal`, `"frame failed" not in journal` — `omoya/flake.nix:489,503`); pass the DRM node into the fixture rather than hardcoding `card0` in three places; add `nix build .#checks.x86_64-linux.vkms-seat` to omoya CI.

**▲ Critique A #9 is right and this is the correction:** `frames > N` + `output_w/h == mode` is passed by a compositor that modesets, flips black forever and drops every client — strictly weaker than today's `"holding the display" in journal` (`flake.nix:482`), which at least reaches presentation. **The counter set must include a captured pixel at a known coordinate matching a client's known colour.** That is what `capture()` (`drm.rs:541`) is for — and it is honest to say it is not free: it has zero call sites and needs `ExportMem` on the renderer (`nuri_renderer.rs:603`). Cost it as part of this phase, not as a Phase-6 nicety.

**Profile cleanup lands here, not in A** — these edits reach plo:
- Delete `nix/flake.nix:703-706` and `profiles/nixos-pleme-omoya/default.nix:96-101` (both say "pixman … libseat"; the second is **rendered into `/etc/pleme/seat.json`**, actively lying to an operator) and `default.nix:317-319`.
- **seatd: measure before flipping.** The draft asserted "libseat is linked by nothing on this seat". omoya's `Cargo.toml:105` says `backend_session_libseat` was **removed**, which supports it — but `profiles/nixos-pleme-omoya/default.nix:290-292` says the opposite in prose, and plo *also* imports the `pleme-desktop` (Hyprland) profile, which does need libseat and is the documented fallback. **Verify against the built closure of both seats, not the source, and record the query.** If Hyprland needs it, seatd stays on and the prose is corrected instead. The failure mode is a black screen.

**Done-predicate:** `vkms-seat` asserts frames advance, mode matches, **and a known pixel reads back the client's colour**; it **red-runs** against a build that modesets and paints nothing. `grep -c card0 omoya/flake.nix` → 0. A `vkms-seat` check-run appears on omoya `main`. `grep -rn "pixman\|libseat" nix/{flake.nix,profiles/nixos-pleme-omoya/}` → 0 stale hits. The seatd decision is recorded with its closure query.

---

### S-4 — Windows you can actually use

**Entry:** S-1 (cursor + focus).

Give `send_configure()` a size; tile or cascade instead of stacking at `(0,0)` (`handlers.rs:109`); implement `move_request`/`resize_request` (`handlers.rs:132-141`); add close and alt-tab. Then output hotplug: re-run connector selection on a DRM uevent (`drm.rs:179 probe()` runs once — `scanout.rs:189-192`'s `commit_pending` claim covers a *mode* change on the same connector, not a *new* one).

**Done-predicate:** two terminals open, both visible, both reachable by keyboard and pointer, either closable from inside the seat. A monitor plugged in mid-session lights up with no restart. **This is the un-pin gate for S-0's leash.**

---

### S-5 — The frame pump

**Entry:** S-3 (you cannot verify pacing you cannot see) + S-4 (single-window damage measurements are a lie).

Pass `DrmDeviceNotifier` into `drm::run`; render the first frame inline (it modesets); move the render body to the `DrmEvent::VBlank` arm; split `DirectScanout::flip` into `submit()` and `presented()`; delete the Timer.

**▲ The watchdog is demoted to a detector.** Critique A #10 is right: re-submitting while a flip is genuinely pending is `-EBUSY` at best and a double-queued flip at worst; "event lost" and "event late" are indistinguishable without the completion sequence the watchdog is meant to protect; it would be written against an invariant that has never run; and vkms's synthetic vblank cannot exercise it. **Land pacing with `missed_flips` plus a hard fallback to the existing timer. Add re-submit only after `missed_flips` has been non-zero on real hardware and the cause is known.**

Then damage — **re-costed L, and the first task is not "add damage"**: `drm.rs:273 pub fn prepare(…)` builds `OutputDamageTracker` and has zero callers, so this is *replacing the live render loop with the one that superseded it*, then slot age (or `Swapchain`), `render_output` with a real age, `FB_DAMAGE_CLIPS` where atomic. Independently: a row-wise `copy_from_slice` fast path in `nuri::Surface::fill`'s opaque branch.

Bootstrap is not a concern — measured: `commit(…, event: true)` generates a flip event on atomic, and legacy queues an extra flip for exactly this reason (`legacy.rs:314-329`).

**Done-predicate:** over 60 s on plo, median `flip_interval_us` within 5% of `1e6/refresh_hz`, `missed_flips == 0`. On an idle two-window desktop, `pixels_painted` per frame < 10% of the framebuffer (today 100%, always). `age = 0` forced on first frame / modeset / resume / VT return / any flip error, proved by a VT round-trip leaving no stale pixels.

---

### S-6 — Input that feels like a desktop

**Entry:** **S-1 (a cursor)** and S-3 (device introspection). *The draft said S-2 + S-3; its real blocker is the pointer being visible.*

Smallest first: stop the touchpad lying (`CAP_POINTER` requires `REL_X`); stable device `id()` from `input_id()`, not `/dev/input/eventN`; hi-res scroll (`REL_WHEEL_HI_RES`, reusing the 120-per-detent convention already present). Then a **new leaf crate** — no smithay, no seat — with `PointerTransfer = Flat { scale } | Adaptive { … }` over `ishou_tokens::Refined`, a pure `apply(raw, dt) -> delta`, shipped with **identity as the default** so landing it is a no-op. Wire at the SYN flush; keep raw in `delta_unaccel`. Flipping the default is a **separate commit with a witnessed session**. Then hotplug: a calloop source owning the netlink udev-group socket, emitting `DeviceAdded`/`DeviceRemoved`, handled where `input.rs:277` says `_ => {}`.

**Done-predicate:** `introspect.input_devices` goes 3 → 2 → 3 across unplug/replug with no restart. The policy crate is green with a mocked environment, zero `/dev` access, zero seat. The default-flip is its own rev with the operator's verdict recorded against it.

**Placement, settled by measurement:** smithay expects the *backend* to do policy — `PointerMotionEvent` requires both `delta_x()` and `delta_x_unaccel()`, its libinput adapter computes nothing, smithay contains zero acceleration code. In omoya the backend is our code.

**awase must not own this.** 7519 lines, zero hits for `accel|tap_to_click|natural_scroll|scroll_method`; its `gesture.rs` is key-chord sequences. Same goal, different shape — write the rule down. mado's `ScrollKinetics` is derivation one; this is derivation two; extract the *shape*, owned by neither, and do not import `ScrollKinetics`.

---

### S-7 — Identity moves into nix

**Entry:** S-3 (the fixture takes a path).

**▲ Order corrected — the draft's step (a) would have left plo with no seat.** `seat-render.nix:78` emits a bare exe and omoya's CLI has **no `--drm-device`** (verified: the only args are `mode` and trailing `spawn`). Emitting the flag first is an unknown clap argument on the greeter's exec line: exit 2, no compositor, on the machine whose local console is the thing under change.

Correct order: **(a′)** land an omoya rev that *accepts and ignores* `--drm-device` → **(b)** bump the input in nix and verify the seat is unchanged → **(c)** nix emits `--drm-device ${scanoutPath}` → **(d)** a later omoya rev makes it required and deletes the `/dev/dri/card*` enumeration. Never (c) and (d) in one commit; never (c) before (a′).

`pleme.nixos.gpu.scanoutPath` **already exists** (`gpu.nix:367`, computed `:411`) — the nix half is S. Same shape for `--layout/--variant/--options`, landing **with** a NixOS assertion that a `pleme-omoya` node's declared layout is in hairetsu's supported set.

**Done-predicate:** `nix eval` of `sessionCommand` contains the by-path node; `grep -rn '/dev/dri/card' omoya/crates/` → 0; the layout assertion **red-runs** on a node declaring `br`, failing at eval naming the machine — not at boot with no seat.

---

### S-8 — Lock and idle

**Entry:** S-4.

`roles.lock = null` is honest and is a security regression against the Hyprland seat it replaced, on a host holding sops-decrypted material. Decide: a pleme-io lock role, or DPMS + idle blank as a floor with the gap written into §7. Do not leave it as one passing mention.

**Done-predicate:** either a lock role in `/etc/pleme/seat.json` with a witnessed lock/unlock, or a §7 row naming the exposure and the date the decision was taken.

---

### S-9 — dmabuf, decided by measurement

**Entry:** S-5 (damage) + S-7 (device plumbing) + S-3 (sensor).

A ~50-line probe on plo first: allocate a linear ARGB8888 image on the nvidia Vulkan device, export via `VK_KHR_external_memory_fd`, `mmap` the fd. **If it fails, the entire CPU-import path is dead** and the answer is GLES/MultiRenderer for session mode with nuri kept as the mandatory CPU tier for `entrance`/`lock`. If it succeeds: implement `import_dmabuf` (mirroring the working `Bind<Dmabuf>` in READ mode, `sync_plane START|READ` / `END|READ` per the pixman oracle) **before** any global; add a dumb-buffer import case to `vkms-seat`; then land the global behind `--dmabuf`, **default off**, with v4 default feedback.

**Done-predicate:** the probe result recorded either way, with the error if it fails. If landed: an A/B table from the sensor — fps, `pixels_painted`, `nvidia-smi` utilization — with and without. **The default flips only if dmabuf wins**, and `theory/OMOYA.md:213`'s parked wgpu-vs-GLES decision is closed by that number.

Default-off is not caution theatre — it is the only thing decoupling a push from a live seat with no human at it.

---

### M-1 — mukae opens the session

**Entry:** S-2 (pause/activate real) **and a written `/etc/shadow` decision.**

Decide first: root-on-VT-then-drop, or a small privileged verifier. `/etc/shadow` is `0640 root:shadow`, the greeter runs as `greeter`. It is a security decision, not a coding one, and everything after depends on it.

**Re-costed, and the draft oversold readiness.** "Written, correct, zero production callers" reads as near-done. Measured: `verify_user` (`mukae-native/src/lib.rs:62`) has **zero** callers; `create_session` (`logind.rs:221`) has only `tests/real_logind.rs`; and `mukae-greeter/Cargo.toml` has **no `mukae-native` dependency at all** — its deps are `mukae-face`, `mukae-spec`, `mukae`, `kanshou`, `egaku-term`, `egaku`, plus linux-only `mukae-greetd` and optional `mukae-host`. This is code no consumer has ever exercised.

Then: add the dependency + a `native` feature; a `NativeSeatEnv` implementing `SeatEnv` (today's only impl is `mock.rs:241`); a non-PAM start path — `create_session` → `/proc/self/loginuid` → `setrlimit` → `setgroups/setgid/setuid` → `execve`. **Extract the `logind` leaf crate here**, since both consumers are now live. Then `SeatMode::Entrance`, which removes greetd and therefore libpam.

**Done-predicate:** `loginctl` shows a graphical session whose creator is mukae; the greeter path's closure carries no `linux-pam`; `/etc/pleme/seat.json`'s citizen fraction rises **because the roster changed**, not because a label was edited; the `mukae`/`regreet` roster divergence disappears by the older surface's answer becoming true.

Deploy trap throughout: `restartIfChanged = false` on greetd, so a broken greeter surfaces at the *next reboot*, decoupled from the deploy that caused it.

---

### S-10 — Destination work

Generated layout corpus behind `(deflayout …)` with the `xkbcli` differential as its 99×496 verification matrix; the cursor plane; overlay/direct scanout with `test_state` before every commit and a typed composite fallback; explicit sync; `DirectSession` as `--session direct`, default off, witnessed on vkms then plo, and **never sold as "logind removed"**.

---

## 7. EXPLICITLY NOT IN THE PLAN

| Not doing | Why |
|---|---|
| **A fleet-wide input-greenness gate** | Deleted. §5 — measured 3/25 green, wrong surface, needs a cross-repo PAT on the push path, and can wedge `nix flake update` fleet-wide. |
| ▲ **XWayland** | Zero hits across the whole omoya tree. **Consequence, stated:** any X11-only application does not start on this seat, at all, indefinitely. Named as a decision rather than left as an omission for the next reader to discover. |
| ▲ **`wp_primary_selection`** | Absent. Middle-click paste — a terminal habit — is dead. `ClientDndGrabHandler`/`ServerDndGrabHandler` are empty impls, so drag-and-drop has no visual. Recorded, not scheduled. |
| **A full XKB implementation / keymap-text parser** | `new_from_string` has one consumer, `new_from_fd` one; omoya calls neither. A parser makes hairetsu a *client* of someone else's keymap, not a server of one. |
| **Reimplementing libinput** | The destination is owning the transfer function as data and the device lifecycle as a reconciler. Re-deriving the device stack is not the reusable part. |
| **`pending-nuri-filtering`** | A no-op today: output scale is 1, no `wp_viewporter`, no fractional-scale protocol, so `src == dst` and nearest is bit-exact with any filter. Belongs *after* the protocol work that makes it matter. Caveat recorded, not fixed: `nuri_renderer.rs:287-293` truncates `Rectangle<f64>` with `as`, a half-pixel shift the day viewporter lands. |
| **`DirectSession` as a default, or framed as "logind removed"** | greetd's `pam_systemd` registers with logind regardless, so on NixOS DirectSession drops the C-daemon count by zero while making the compositor root. The row that removes logind is M-1. Keep the wire, own the executor. |
| **Overlay planes / direct scanout before S-5 + S-9** | The only change in the set that can turn a working seat into a black screen, and it presupposes damage (to know what to promote) and completion events (to know when a plane's buffer is free). |
| **Deleting retired surfaces** (regreet, seatd, blzsh leftovers) | ★★ MODULARIZE, DON'T DELETE. Flip the typed `enable` off; the declaration stays. |
| **Multi-seat / seat1** | plo is one seat. |
| **xkb *actions* (`LockGroup`, `MovePtr`, `SetGroup`)** | Declared unsupported rather than half-emitted. Upstream `us` carries 53 `MovePtr` interpret blocks; a partial emission would compile and misbehave. |
| **Anything landing on plo before S-0's pin** | Not a schedule preference — a live DNS server and subnet router whose only local console is the thing being changed. |

---

## 8. THE SINGLE HIGHEST-LEVERAGE NEXT ACTION

**Re-establish ssh to plo, run the S-0 probe, and set `pleme.gitops.source.branch = "seat-staging"` in the same session.**

Read-only for the probe, one typed field for the pin. It resolves seven unknowns (reachability, VT escape, suspend/resume, atomic-vs-legacy, the by-path node, the *running* gitops backend, whether DNS and the subnet route survive a seat failure) and then **cuts the leash**: every phase after it stops being one panic-on-startup rev away from a machine recoverable only from the boot menu, in person, with no console because the RTX 3070 is also the firmware's primary adapter.

**Start Stream A in the same breath — it needs no plo.** Fix omoya's `Test gate` and mukae's `Test gate` + `build`, then land `cargo metadata --locked` and the `flake.lock` → `plo.toplevel` build gate. That is the compounding half, and it is now scoped to a predicate that is actually true rather than to 154 repos of which 3 in 25 are green.

**And the moment ssh is back, S-1 is two lines and a cursor.** Today a successful login on plo produces Nord0, no pointer, no program, and no second window. Every measurement in S-5 and S-9 is a measurement of a screen nobody can use.

---

## WHAT CHANGED AFTER REVIEW

**Adopted from the skeptic (A) — 11 of 13:**

1. **Blast radius named, and partially corrected.** A is right that the draft understated it and right about DNS + the subnet route (`nodes/plo/tailscale.nix:11`, `configuration.nix:240`). A is **wrong that plo is the k3s control plane**: `configuration.nix:318` sets `pleme.nixos.k3s.enable = false` — stood down 2026-08-18, FluxCD followed at `:459-462`. A quoted the developer-profile comment at `default.nix:33-36` and missed the supersession at `:50-58` in the same file. The blast radius is DNS + route + the console SPOF (no iGPU, `boot_vga=1`), not the cluster. New §0; measured rows added to S-0.
2. **The greenness gate is deleted, not repaired.** A's counts re-measured and confirmed: **165 pleme-io github nodes / 154 unique repos**, not ~330 (393 total github nodes is where the figure came from). Replaced with an advisory `input-health` report plus a narrow `flake.lock` → `plo.toplevel` build gate. §5 is new.
3. **The RAlt fix was prescribed in the wrong file and its test could not catch the drift it cited.** Confirmed: `lib.rs:221` and `emit.rs:63` both say Mod5 — they **agree**, there is no RAlt drift. The real drift is Scroll Lock (`emit.rs:57,107` vs `lib.rs:227-234`, no `Scroll_Lock` arm). New §4; the parity test is re-keyed to the modmap seam.
4. **Name-keyed → keysym-keyed.** Confirmed: `layout.rs:157` gives RALT `Alt_R` in `us`; br gives it `ISO_Level3_Shift`; `lib.rs:426` reads MOD5 for level 3. The draft's item #8 would have been undone by its own item #20.
5. **S-7's sequence inverted.** Confirmed: omoya's CLI has no `--drm-device`, so the draft's step (a) is exit 2 on the greeter line. `scanoutPath` already exists (`gpu.nix:367,411`) — nix half re-costed M→S.
6. **Phase 1's profile edits moved out.** They touch the profile plo imports; the phase was mislabelled "no plo dependency". Now S-3, and the seatd flip must be measured against the built closure of *both* seats first.
7. **Auto-deploy demoted from premise to probe row.** `configuration.nix:637` *declares* sentinela; `:625-628` says the handover is out-of-band via ssh, which is failing.
8. **Dead code re-costed.** Confirmed: `drm.rs:273 prepare(` zero callers; `capture(` zero call sites; `verify_user` zero callers; `create_session` test-only; `mukae-greeter/Cargo.toml` has **no** `mukae-native` dep. Damage re-costed S→L; M-1's readiness claim removed.
9. **S-3's done-predicate no longer passes on a black screen.** Pixel readback added, with the honest cost (`ExportMem`) named.
10. **The watchdog is a detector.** Re-submit deferred until `missed_flips` is non-zero on real hardware.
11. **Split into three streams** with separate entry conditions.
12. **Three round-ups removed** — "one root cause wearing three costumes" (now three defects, three witnesses), "~330 inputs", and item #4's cost.

**Rejected, with evidence — 1 of 13:**

- **A #13 ("the header claims tearing impossible rather than unlikely").** `grep -rn "tearing"` across omoya returns **exactly one hit**, `scanout.rs:61`, which makes no such claim. The header's actual admission is "full repaint every frame — the damage rectangles are computed and then ignored", with `pending-omoya-damage` attached. Both the draft's accusation and A's correction target prose that does not exist; the row is deleted rather than reworded. A's underlying point — that `scanout.rs:198-200` is honestly commented — is correct and is why the round-up claim had to go.

**Adopted from the operator (B) — all 10, and it reordered the plan:**

- **The verdict is accepted in full: the draft optimized a desktop nobody can use.** Three items that did not exist in the draft are now #1–#3 by leverage: **the deploy-source pin** (one typed field, `modules/pleme/shared/gitops.nix:61`), **a launcher** (`seat-render.nix:78` renders a bare exe; omoya's spawn is trailing argv), and **a cursor plus a keyboard focus path** (`handlers.rs:228` empty, `input.rs:187-196` pointer-only). New phase S-1.
- **Window management** promoted from absent to S-4 and made the un-pin gate (`handlers.rs:109,132-141`).
- **Suspend/resume** added as a second required witness in S-2 — same code path, higher frequency.
- **Output hotplug** added as item #19, stated precisely: `commit_pending` covers a mode change on the same connector; nothing re-runs `probe()`.
- **Lock/idle** promoted from one passing mention to its own phase with a forced decision (`roles.lock = null`, `default.nix:216`).
- **AltGr severity raised** to "cannot type `@ \ | ~ { }`", which is what it is.
- **Clipboard + primary selection + XWayland** added — clipboard as a measured row in S-0, the other two as §7 rows with their consequences spelled out.
- **seatd flip** blocked on a closure query rather than an assertion, and moved off the plo-free phase.
- **Waivers are per-input, not per-run** — moot for the deleted gate, retained for `input-health`.
- **S-6's entry condition corrected** from S-2+S-3 to S-1: its real blocker is a visible pointer.
