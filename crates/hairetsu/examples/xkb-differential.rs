//! Differential every registered layout against **real libxkbcommon**.
//!
//! ── ★ WHY THIS IS AN ORACLE AND NOT A SECOND OPINION ──────────────────────
//!
//! `xkbcli compile-keymap` is the authoritative implementation reading
//! xkeyboard-config's own data. When our emitted keymap and its output
//! disagree about a key's symbol list, **we are wrong** — there is no case
//! where the disagreement means xkeyboard-config mis-transcribed the Brazilian
//! ABNT2 keyboard.
//!
//! That asymmetry is what makes this worth running rather than reasoning
//! about. Our layout tables are hand-transcribed from `symbols/br` and
//! `symbols/latin`, and hand-transcription fails silently: a wrong keysym
//! still produces *a* character, so the seat comes up and one key is quietly
//! wrong until somebody types it.
//!
//! ── ★ WHAT IT ALREADY FOUND ───────────────────────────────────────────────
//!
//! On its first run (2026-09-09) it caught `<TAB>` emitting a one-level `Tab`
//! where upstream emits `[ Tab, ISO_Left_Tab ]` — so Shift+Tab produced a
//! plain Tab and every reverse-cycle binding in a shell or TUI silently went
//! forwards. That defect was in the `us` table since the crate was written and
//! had never been noticed, because the key *worked*.
//!
//! ── ★ RUNNING IT ──────────────────────────────────────────────────────────
//!
//! ```text
//! cargo run -p hairetsu --example xkb-differential
//! ```
//!
//! Needs a Linux host with `xkbcli` (libxkbcommon) on PATH. It is NOT a
//! `#[test]`, deliberately: a test that skips when `xkbcli` is absent goes
//! green on every machine that cannot run it, which is the vacuous-gate shape
//! this crate's own matrix exists to refuse. Exiting **2** on a missing oracle
//! is the honest answer — "not checked" is not "checked and fine".
//!
//! Exit codes: `0` no unexpected difference · `1` a difference · `2` no oracle.

use std::collections::BTreeMap;
use std::process::Command;

/// Keys where upstream deliberately carries levels a Wayland compositor does
/// not want, with the reason. Anything NOT in here that differs is a defect.
///
/// ★ Each entry is a decision with a stated reason, not a suppression. The two
/// classes are genuinely different and are kept apart on purpose:
///
/// * `XFree86Vt` — X's own VT-switch and video-mode keysyms. omoya recognises
///   Ctrl+Alt+F<n> in `input.rs` and calls `change_vt` itself; it does not
///   route VT switching through a keysym the way an X server does. Emitting
///   `XF86Switch_VT_4` would advertise a capability the seat does not
///   implement through that path.
/// * `XServerGrab` — `XF86Ungrab` / `XF86ClearGrab` break an X server's active
///   grab. There is no such thing to break on Wayland.
/// * `Owed` — a real level we do not emit yet, kept HERE rather than silently
///   dropped so the list reads as a to-do and not as an approval.
const EXPECTED: &[(&str, Reason)] = &[
    ("FK01", Reason::XFree86Vt),
    ("FK02", Reason::XFree86Vt),
    ("FK03", Reason::XFree86Vt),
    ("FK04", Reason::XFree86Vt),
    ("FK05", Reason::XFree86Vt),
    ("FK06", Reason::XFree86Vt),
    ("FK07", Reason::XFree86Vt),
    ("FK08", Reason::XFree86Vt),
    ("FK09", Reason::XFree86Vt),
    ("FK10", Reason::XFree86Vt),
    ("FK11", Reason::XFree86Vt),
    ("FK12", Reason::XFree86Vt),
    ("KPAD", Reason::XFree86Vt),
    ("KPSU", Reason::XFree86Vt),
    ("KPDV", Reason::XServerGrab),
    ("KPMU", Reason::XServerGrab),
    ("PAUS", Reason::Owed),
    ("PRSC", Reason::Owed),
    // MENU has no upstream counterpart under these RMLVO names.
    ("MENU", Reason::NotInOracle),
    ("BKSP", Reason::Redundant),
    ("LSGT", Reason::UnreachableUpstream),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    XFree86Vt,
    XServerGrab,
    Owed,
    NotInOracle,
    Redundant,
    UnreachableUpstream,
}

impl Reason {
    const fn why(self) -> &'static str {
        match self {
            Self::XFree86Vt => {
                "upstream's XF86Switch_VT_n / XF86*_VMode levels — omoya owns \
                 VT switching in input.rs, not through a keysym"
            }
            Self::XServerGrab => "XF86Ungrab / XF86ClearGrab break an X grab; Wayland has none",
            Self::Owed => {
                "OWED — a real level we do not emit (Print/Sys_Req needs an \
                 ALT_LEVEL2 key type, Pause/Break a CONTROL_LEVEL2 one). \
                 pending-hairetsu-modifier-level-types"
            }
            Self::NotInOracle => "no upstream counterpart under these RMLVO names",
            Self::Redundant => {
                "upstream declares two levels whose keysyms are IDENTICAL \
                 (`BackSpace, BackSpace`); one level is behaviourally the same \
                 and says so"
            }
            Self::UnreachableUpstream => {
                "upstream gives <LSGT> four levels (`bar`, `brokenbar` at 3/4) \
                 while binding RALT to plain Alt_R — verified 2026-09-09 — so \
                 those levels are UNREACHABLE on stock `us`. Declaring them \
                 would also fail our own matrix rule that a four-level table \
                 must bind an ISO_Level3_Shift. `br` DOES bind one, and there \
                 <LSGT> is four-level and matches upstream exactly"
            }
        }
    }
}

fn syms(block: &str) -> Option<String> {
    // Both formats appear: ours writes `symbols[Group1]= [ .. ]` on one line,
    // xkbcli writes it across lines, and some of its keys carry a bare
    // `[ .. ]` with no `symbols[N]=` prefix at all.
    let after = block
        .find("symbols")
        .and_then(|i| block[i..].find('=').map(|j| i + j))
        .map_or(block, |i| &block[i..]);
    let open = after.find('[')?;
    let close = after[open..].find(']')? + open;
    Some(
        after[open + 1..close]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect(),
    )
}

/// `key <NAME> { .. };` → symbol list, for either emitter's spelling.
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = text;
    while let Some(i) = rest.find("key <") {
        rest = &rest[i + 5..];
        let Some(gt) = rest.find('>') else { break };
        let name = rest[..gt].to_owned();
        let Some(open) = rest[gt..].find('{') else {
            break;
        };
        let Some(close) = rest[gt + open..].find('}') else {
            break;
        };
        let block = &rest[gt + open..gt + open + close];
        if let Some(s) = syms(block) {
            out.insert(name, s);
        }
        rest = &rest[gt + open + close..];
    }
    out
}

fn main() {
    let expected: BTreeMap<&str, Reason> = EXPECTED.iter().copied().collect();
    let mut unexpected = 0_usize;
    let mut compared = 0_usize;

    for def in hairetsu::layout::LAYOUTS {
        let ours = parse(
            hairetsu::Keymap::for_layout(def.rmlvo)
                .expect("registered")
                .as_text(),
        );

        let oracle = Command::new("xkbcli")
            .args([
                "compile-keymap",
                "--rules",
                "evdev",
                "--model",
                "pc105",
                "--layout",
                def.rmlvo,
            ])
            .output();
        let Ok(out) = oracle else {
            eprintln!(
                "no `xkbcli` on PATH — the oracle is unavailable, so NOTHING was \
                 checked. Exiting 2 rather than 0: \"not checked\" is not \
                 \"checked and fine\"."
            );
            std::process::exit(2);
        };
        if !out.status.success() {
            eprintln!(
                "xkbcli refused layout {}: {}",
                def.rmlvo,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            std::process::exit(2);
        }
        let theirs = parse(&String::from_utf8_lossy(&out.stdout));

        let (mut same, mut waived) = (0_usize, 0_usize);
        let mut bad: Vec<String> = Vec::new();
        for (name, mine) in &ours {
            compared += 1;
            match theirs.get(name) {
                Some(t) if t == mine => same += 1,
                other => {
                    if expected.contains_key(name.as_str()) {
                        waived += 1;
                    } else {
                        bad.push(format!(
                            "  {name}\n    ours   : {mine}\n    oracle : {}",
                            other.map_or("<absent>", String::as_str)
                        ));
                    }
                }
            }
        }
        println!(
            "{:<3} keys={:<4} identical={:<4} waived={:<3} UNEXPECTED={}",
            def.rmlvo,
            ours.len(),
            same,
            waived,
            bad.len()
        );
        for b in &bad {
            println!("{b}");
        }
        unexpected += bad.len();
    }

    // ★ Anti-vacuity. A parse regression on either side would empty both maps
    // and report a clean run over nothing.
    assert!(
        compared > 150,
        "only {compared} keys compared — the parser is not reaching the tables"
    );

    for (name, reason) in &expected {
        println!("waived {name}: {}", reason.why());
    }

    if unexpected > 0 {
        eprintln!(
            "\n{unexpected} unexpected difference(s) from libxkbcommon. Each is \
             a defect in OUR table until shown otherwise — the oracle reads \
             xkeyboard-config's own data. If a difference is genuinely correct \
             for a Wayland seat, add it to EXPECTED with a stated reason."
        );
        std::process::exit(1);
    }
    println!("\nno unexpected difference from libxkbcommon.");
}
