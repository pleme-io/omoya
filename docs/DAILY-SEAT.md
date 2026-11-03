# The daily seat: omoya on zek, with blue driving what a person customizes

Written before the code, as the `bidama` skill section 0 asks. Each claim was read
from source at omoya b3c49fa and blue ab4e477 on 2026-11-03. `tobira/docs/BLUE-DRIVES-TOBIRA.md`
is the model.

## 1. Destination

omoya is a laptop session that one person can live in all day. A person's world says
what the seat is, entry by entry. A bad entry is refused alone, and its siblings stand.
A world that fails whole leaves the seat on what stood. Saving a file takes effect at
once. Rust keeps what blue cannot reach: the frame loop, the kernel wires, the Wayland
protocol, and the supervision of blue itself.

GNOME stays the default login on zek until the operator says otherwise. Nothing here
flips it.

## 2. What a survey claimed, measured again

| Claim at b3c49fa | Measured | Verdict |
|---|---|---|
| Lock and idle absent, smithay 0.7.0 has a bypass | `session_lock/lock.rs:181-189` posts `InvalidUnlock` and then calls `state.unlock()`. `mod.rs:148` mints a fresh `lock_status = false` per request. | ✅ true |
| No DPMS | No DPMS property write in `crates/omoya/src` | ✅ true |
| No notification daemon | No `org.freedesktop.Notifications` in `crates/` | ✅ true |
| `Deed` is closed and carries no command | `deed.rs:52`, by design: the verb surface must not become a remote shell | ✅ true, and the design stays |
| No media keys | Worse. `nazoru::caps::classify` opens a device as a keyboard only with `KEY_A`, so the ACPI `Video Bus` node that carries brightness keys is never opened. `chord::key_from` has no media arm, and hairetsu's tables have no media keys. | ✅ true, three causes |
| Sleep is logind only, resume unwitnessed | `logind.rs` listens on the session path only. `PrepareForSleep` is on the manager path. | ✅ true |
| A named variant is refused | `xkbcommon-hairetsu` `new_from_names` | ✅ true |
| No screencopy, no chord | `omoya_capture` writes a PPM to a path, on request over kanshou | ✅ true |
| One clipboard plane | `data_device` only | ✅ true |
| Touchpad code is unproven | `pending-nazoru-witness` | ✅ true, his hands only |

## 3. How blue reaches the seat

**Decision: a supervised child, the same binary, exactly tobira's shape.** No blue call
ever runs on the render thread. The compositor has one thread that owns the frame loop,
and a blue value reaches it only as a finished, typed message on a calloop channel.

| Option | Verdict | Why |
|---|---|---|
| In-process embed | ❌ | A deep recursion or a huge allocation aborts the process (`blue-lang-runtime/src/memory.rs`). Here the process is the seat. |
| A `blue` binary on PATH | ❌ | A version the bidama does not match, and no host surface. |
| The same binary as a child | ✅ | An abort, an out-of-memory or a hang ends one child. The parent reads EOF or passes a deadline and says so. |

**Reuse before a second copy.** tobira holds the wire, the worker loop and the pool
privately. A second copy is a bug, so they move to one crate, `blue-lang-worker` in
`pleme-io/blue`. omoya is its first consumer. tobira moves on its next refactor.

## 4. The reuse map

| Need | Exists | Use |
|---|---|---|
| Embedding | `blue_lang_host::{Program, Budget, Capability}` | as is |
| Worker, wire, deadlines | tobira `src/world/{pool,wire,worker}.rs` | moved to `blue-lang-worker` |
| Entries refused alone | `kousei::judge` | the world's declaration |
| The schema | `OmoyaConfig`, serde `deny_unknown_fields` | one trial merge per field. Rust stays the one schema, and blue holds no second field table |
| Key words | `awase::Hotkey::parse`, `awase::Key::VolumeUp` and its siblings | a binding's chord. `Reserved::fleet_linux` refuses a chord that would eat the VT escape |
| Live config | `config::load` | the base under the world |
| Bar zone | `bar_modules::Module::ALL` | gains one blue row |
| Modal state | `presence::Presence` | gains a rest state |
| Frame gate | `mekuri::Gate<Owed>` | gains `Owed::Lock` |
| logind | `logind.rs`, zbus | gains the manager path |
| Session lock wire | `wayland_protocols::ext::session_lock` | our own dispatch. See section 7 |
| Idle inhibit, primary selection, data control | smithay delegates | as is |
| Screencopy wire | `wayland_protocols_wlr::screencopy` | our own dispatch |
| Layout registry | `hairetsu::layout::LAYOUTS` | gains a variant column |

## 5. The shape

### The world: `~/.config/omoya/omoya.b`

```blue
use("kurashi", %i[backlight bar_module bind command idle keyboard lock_with look notes run sleep swaylock volume])

def omoya()
  [
    keyboard(%w[us us+intl]),
    look({accent: :frost2, bar_height: 26}),
    idle({blank_s: 300, lock_s: 600}),
    sleep({lock_before: true}),
    lock_with(swaylock(:nord)),
    notes({ms: 6000, max: 3}),
    command(:louder, volume(5)),
    command(:brighter, backlight(5)),
    bind("volume_up", :louder, {locked: true}),
    bind("brightness_up", :brighter, {locked: true}),
    bind("super+shift+s", :screenshot),
    bar_module("uptime", {every_s: 60, allow: [:filesystem]})
  ]
end
```

The host calls `omoya()` in a child, passes the list through `kurashi::declare`, and
reads `accepted`, `off` and `refusals`. `look`, `idle`, `sleep`, `lock_with`, `notes`
and `keyboard` lower to `config(group, fields)`, so one trial merge per field checks
them against the same Rust schema the YAML has. YAML keeps working. A world lays over it.

### A bar module: `~/.config/omoya/bar/uptime.b`

```blue
def render()
  "#{floor(uptime_s / 3600)}h"
end
```

A string is shown. `nil` shows nothing, the zone's rule that a module earns its place.

### Named commands open `Deed`, by name only

`Deed::Command(name)` carries a name, never an argv. The table of what a name does is
the world's, on the seat. `omoya_do command:louder` over kanshou can run only what the
person declared, so the verb surface stays no remote shell. An undeclared name is a
refusal that says so.

### The wire

JSON lines over the child's stdin and stdout, one request at a time.

```text
→ {"id":7,"call":"render","args":[]}
← {"id":7,"value":"3h"}
← {"id":7,"refused":{"kind":"exhausted","why":"render: ..."}}
← {"load_refused":{"kind":"load","why":"parse error at 3:1"}}
```

### Failure, each with its answer

| Fault | Who sees it | Answer |
|---|---|---|
| The world fails to parse or raises | the world child | `world_state: refused`, the seat stays on what stood |
| One entry is malformed | `kousei::judge` | refused alone, listed in `world_refusals` |
| One field fails the schema | the trial merge | refused alone by path |
| A chord would eat `Ctrl+Alt+F<n>` | `Reserved::claim_on` | refused alone |
| A world that hangs | 10 s deadline, the parent kills it | as a refused world |
| A module raises or loops | `CallError` in the child | its bar cell empties, and `world_refusals` names it |
| A module hangs past its hard deadline | the parent kills it | a new child at its next tick |
| An abort or an out-of-memory | EOF on the pipe | as a refused module, a new child at its next tick |

The bar's right zone never blocks a frame. It reads the last text each module sent.

## 6. Why each piece is blue or Rust

| Piece | Rung | Why |
|---|---|---|
| The world, its words, a person's look | blue (`kurashi`) | A declaration a person edits. Pure constructors. |
| Bar modules, a note's rule | blue | Logic a person writes. A vigy on a tick. |
| The swaylock argv from the look | blue | A rendering of the world. |
| The worker and its wire | Rust | The seam that hosts blue. No rung above can supervise itself. |
| Frame loop, DRM, evdev, Wayland dispatch | Rust | A system call and a protocol, on the render thread. |
| The lock state machine | Rust | It decides what a screen shows. A type, not a value a world can reach. |
| Idle policy | Rust, knobs from blue | It reads the input clock on the frame thread. The numbers are the world's. |
| Backlight step | Rust | A logind call on the seam the seat already speaks. |
| Volume | Rust argv to `wpctl`, knob from blue | A process boundary. A bare seat with no world keeps its keys. PipeWire's native protocol has no Rust surface here, and `pending-omoya-pipewire` names it. |
| Notification wire | Rust child | A D-Bus name. It lives in a child so a bad message cannot reach the frame loop. |

## 7. The lock, because it is security

**Client: `swaylock`, from nixpkgs, run by the seat and rendered from the world.** The
seat speaks `ext-session-lock-v1`, so any client works. `mukae --mode lock` is the
destination (`DESKTOP-PLAN.md` P4), and it has no lock face today. `pending-omoya-lock-face`.

**smithay's `session_lock` is not used.** 0.7.0 holds the bypass in section 2, and master
is in no release. A `[patch]` git pin moves all of smithay under a live seat. So omoya
speaks the wire itself over `wayland_protocols`, in `session_lock.rs`, and the decision
is a pure machine in `omoya-spec`, tested on a Mac.

```text
Open ── lock request, or a compositor lock ──▶ Held
Held { holder: Some(lock), announced }
Held { holder: None }        ◀── the holder died: the screen stays locked
Held ── unlock_and_destroy from the announced holder only ──▶ Open
```

| Rule | Why |
|---|---|
| The scene is decided once per frame from `hides_desktop()`, after every other element is gathered, and it replaces the whole list | A new element added later cannot leak past a lock. |
| `locked` is sent only after a lock frame retired on the screen | The protocol's race: a client that suspends on `locked` must not race the first locked frame. |
| A compositor lock enters `Held { holder: None }` first and starts the client second | A client that is slow, missing or broken cannot delay the blank. |
| An unlock from any object but the announced holder is a protocol error and changes nothing | The bypass, closed by the type of the transition. |
| A second `lock` while a holder lives gets `finished` | One holder. |
| A `lock` while `holder: None` takes over | A dead client must not strand the person. |
| A dead holder respawns the client, with backoff | There is no other way back on a laptop. |
| While `Held`, a key reaches only a lock surface, a pointer finds only a lock surface, and a deed needs an explicit `locked` flag | Input is the other half of a lock. |
| Screencopy and the screenshot chord refuse while `Held` | The lock frame is no one's to read. |

**The way back when no client can start:** switch VT, log in, and run a client against
the seat's socket: `WAYLAND_DISPLAY=wayland-1 swaylock`. It takes over `holder: None` and
unlocks the normal way. No kanshou verb can unlock, by design.

## 8. Sleep

The seat holds a logind `delay` inhibitor. On `PrepareForSleep(true)` it locks, and it
releases the inhibitor when a lock frame has retired, or at once if the panel is blank
or another VT holds the seat. On `PrepareForSleep(false)` it takes a new inhibitor, and
counts the resume, and restores the panel through the same path as a VT return. A lid
is logind's: `HandleLidSwitch` starts the same sleep. `Lock` and `Unlock` on the session
object lock the seat, and `Unlock` is ignored, because only the client's proof unlocks.

## 9. Idle and the panel

The clock is the last input. The policy is two numbers from the world, `blank_s` and
`lock_s`. An idle inhibitor (`zwp_idle_inhibit_manager_v1`, a playing video) holds both.

To blank, the seat presents one black frame first and then sets the connector's DPMS
property off. So a wake never shows old pixels, whatever happened while dark: DPMS on
shows black, and the next frame is the current scene. A key press that wakes the panel
is swallowed with its release, so a person cannot type blind into a focused window.

## 10. Notifications: a blue surface, not shirase

shirase has no Linux source, no Linux wire and a GPU panel (`shirase/CLAUDE.md`,
`platform/linux` planned). Growing it would put a GPU client and a layer-shell surface
under the first banner, on a seat whose clients stay on CPU buffers. A banner here is a
rectangle and a line of text, and the seat already draws both with `fontdue` and `nuri`,
in his look, with no client that can fail to start.

So `omoya notes-worker`, a child, owns `org.freedesktop.Notifications` on the session
bus. It runs an optional `~/.config/omoya/notes.b` (`def note(n)` returns the note, a
changed note, or `nil` to drop it), and it writes finished notes to the seat as JSON
lines. The seat draws them. A dead child is respawned and loses only its queue. shirase
keeps the history and the panel on the Mac, and it can read the same wire later.

A note never draws while `Held`.

## 11. The idioms

- One namespace per bidama. `kurashi::bind`, or listed in `use`. No prefix.
- A constructor is pure and returns a record.
- A malformed entry is data for `kousei::judge`, never a raise.
- The four tests, each red once: empty, identity, an independently checked value, a control.
- `@moduledoc`, and `@doc` with a doctest on every definition.
- Rust errors are typed. New Rust carries no comments.

## 12. The order

Each step lands on `main` when green, so a stop anywhere leaves working code.

1. `kurashi` and `blue-lang-worker` land in `pleme-io/blue`.
2. The seam: `omoya blue-world`, `omoya blue-module`, world apply, hot reload, `Deed::Command`, bar modules, look.
3. `us+intl` and layout switching. Media keys reach the seat.
4. Brightness and volume.
5. Idle and DPMS.
6. Lock, with the crash test.
7. Lid and suspend.
8. Notifications.
9. Screenshot chord and screencopy.
10. Primary selection and data control.

**Not in this list:** XWayland, HDR, IME, multiple monitors, portals.

## 13. What is built, by tier

Updated as each step lands.

| Piece | Tier |
|---|---|
| This plan | written |
