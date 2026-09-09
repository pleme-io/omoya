//! Print the emitted XKB keymap, so it can be compiled by a real XKB
//! implementation as a differential check on the wire format.
//!
//! ```text
//! cargo run -p hairetsu --example dump-keymap            # us
//! cargo run -p hairetsu --example dump-keymap -- br      # a registered layout
//! ```
//!
//! ★ The differential this exists for, spelled out because the value is in
//! running it rather than in having it:
//!
//! ```text
//! cargo run -q -p hairetsu --example dump-keymap -- br > ours.xkb
//! xkbcli compile-keymap --rules evdev --model pc105 --layout br > theirs.xkb
//! # then compare the `key <NAME> { [ ... ] };` lines
//! ```
//!
//! `xkbcli` is the authoritative implementation reading xkeyboard-config's own
//! data, so a disagreement on a symbol line is OUR table being wrong — this is
//! an oracle, not a second opinion. It needs a Linux host with libxkbcommon;
//! the crate itself needs neither.
fn main() {
    let mut args = std::env::args().skip(1);
    let layout = args.next().unwrap_or_else(|| "us".to_owned());

    let Some(keymap) = hairetsu::Keymap::for_layout(&layout) else {
        let known: Vec<&str> = hairetsu::layout::LAYOUTS.iter().map(|l| l.rmlvo).collect();
        eprintln!(
            "unknown layout {layout:?} — hairetsu registers {known:?}.\n\
             Refusing rather than falling back to `us`: a keymap dumped under \
             the wrong name is exactly the silent substitution this crate exists \
             to prevent, and it would make the differential above confirm a \
             layout nobody asked for."
        );
        std::process::exit(2);
    };
    print!("{}", keymap.as_text());
}
