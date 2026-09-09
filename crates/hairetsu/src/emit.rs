//! XKB keymap *text* emission.
//!
//! This is the one output of this crate that leaves the process: Wayland hands
//! clients a keymap string, and they compile it with whatever XKB
//! implementation they have. So this text is a **wire format**, and the rule
//! for a wire is to speak it exactly rather than approximate it.
//!
//! Two deliberate choices:
//!
//! * **Self-contained** — no `include "complete"`. An include would make our
//!   keymap depend on the client having xkeyboard-config's data files
//!   installed, which is exactly the foreign dependency this crate exists to
//!   remove. Everything is written out.
//! * **Real modifiers only** — no `virtual_modifiers`. Virtual modifiers have
//!   to be *bound* through `interpret` rules before a type can use them, and a
//!   mis-bound virtual modifier fails quietly (the key just does nothing).
//!   `Mod2` says what `NumLock` means with no resolution step.

use crate::layout::KeyEntry;
use std::fmt::Write as _;

/// Render a keysym the way XKB's parser expects to read it back.
///
/// `xkeysym::Keysym::name` returns the **X11 C header macro** name — `XK_a`,
/// `XK_KP_Home`, `XF86XK_AudioMute` — while XKB keymap text wants the bare
/// symbol: `a`, `KP_Home`, `XF86AudioMute`. Emitting xkeysym's spelling
/// produces a keymap that reads plausibly and that every client rejects, so
/// the prefixes are stripped here.
///
/// Measured, not assumed: `XF86_AudioMute` was the guess, `XF86XK_AudioMute`
/// is what the crate actually returns.
fn keysym_name(raw: u32) -> String {
    if let Some(name) = xkeysym::Keysym::new(raw).name() {
        // Order matters: `XF86XK_` also starts with no `XK_`, but a future
        // `XK_`-first check would mis-handle any vendor prefix, so vendor
        // prefixes are tested first.
        if let Some(rest) = name.strip_prefix("XF86XK_") {
            return format!("XF86{rest}");
        }
        if let Some(rest) = name.strip_prefix("XK_") {
            return rest.to_owned();
        }
        return name.to_owned();
    }
    // ★ A Unicode-form keysym gets XKB's `U<HEX>` spelling, not the numeric
    // one. Both compile — this was found by differential against
    // `xkbcli compile-keymap --layout br` on 2026-09-09, which agreed with us
    // on 84 of 88 comparable keys and disagreed here only in spelling
    // (`U2022` vs `0x01002022`, `U1E9E` vs `0x01001e9e`).
    //
    // Fixed anyway, because a wire format is read by people as well as
    // parsers: `U2022` says BULLET to anyone who knows Unicode, and
    // `0x01002022` says nothing until you subtract the 0x01000000 prefix. The
    // whole reason this module exists is that emitting a plausible-looking
    // keymap is not the same as emitting the right one.
    if let Some(cp) = raw.checked_sub(0x0100_0000)
        && (0x0100..=0x0010_FFFF).contains(&cp)
    {
        return format!("U{cp:04X}");
    }
    // No name and not a Unicode keysym: the numeric form XKB always accepts.
    format!("0x{raw:08x}")
}

/// Which real modifier a key contributes, for `modifier_map`.
fn modmap_entry(entry: &KeyEntry) -> Option<&'static str> {
    Some(match entry.name {
        "LFSH" | "RTSH" => "Shift",
        "CAPS" => "Lock",
        "LCTL" | "RCTL" => "Control",
        "LALT" => "Mod1",
        "NMLK" => "Mod2",
        "SCLK" => "Mod3",
        "LWIN" | "RWIN" => "Mod4",
        "RALT" => "Mod5",
        _ => return None,
    })
}

/// The fixed type definitions.
///
/// These encode the same level rules `State::level_for_key` applies, in XKB's
/// vocabulary. If one side changes, the other must — that is the seam this
/// module cannot make unrepresentable, and the parity test is what guards it.
const TYPES: &str = r#"xkb_types "hairetsu" {
    type "ONE_LEVEL" {
        modifiers= none;
        map[none]= Level1;
        level_name[Level1]= "Any";
    };
    type "TWO_LEVEL" {
        modifiers= Shift;
        map[Shift]= Level2;
        level_name[Level1]= "Base";
        level_name[Level2]= "Shift";
    };
    type "ALPHABETIC" {
        modifiers= Shift+Lock;
        map[Shift]= Level2;
        map[Lock]= Level2;
        level_name[Level1]= "Base";
        level_name[Level2]= "Caps";
    };
    type "KEYPAD" {
        modifiers= Shift+Mod2;
        map[None]= Level1;
        map[Mod2]= Level2;
        map[Shift+Mod2]= Level1;
        level_name[Level1]= "Base";
        level_name[Level2]= "Number";
    };
    type "PC_ALT_LEVEL2" {
        modifiers= Mod1;
        map[None]= Level1;
        map[Mod1]= Level2;
        level_name[Level1]= "Base";
        level_name[Level2]= "Alt";
    };
    type "PC_CONTROL_LEVEL2" {
        modifiers= Control;
        map[None]= Level1;
        map[Control]= Level2;
        level_name[Level1]= "Base";
        level_name[Level2]= "Control";
    };
    type "FOUR_LEVEL" {
        modifiers= Shift+Mod5;
        map[None]= Level1;
        map[Shift]= Level2;
        map[Mod5]= Level3;
        map[Shift+Mod5]= Level4;
        level_name[Level1]= "Base";
        level_name[Level2]= "Shift";
        level_name[Level3]= "Alt Base";
        level_name[Level4]= "Shift Alt";
    };
    type "FOUR_LEVEL_ALPHABETIC" {
        modifiers= Shift+Lock+Mod5;
        map[None]= Level1;
        map[Shift]= Level2;
        map[Lock]= Level2;
        map[Mod5]= Level3;
        map[Shift+Mod5]= Level4;
        map[Lock+Mod5]= Level4;
        map[Shift+Lock+Mod5]= Level3;
        level_name[Level1]= "Base";
        level_name[Level2]= "Shift";
        level_name[Level3]= "Alt Base";
        level_name[Level4]= "Shift Alt";
    };
};"#;

/// Compatibility rules: how held keys become modifier state, and the LEDs.
const COMPAT: &str = r#"xkb_compatibility "hairetsu" {
    interpret.useModMapMods= AnyLevel;
    interpret.repeat= False;
    interpret Caps_Lock+AnyOfOrNone(all) {
        action= LockMods(modifiers=Lock);
    };
    interpret Num_Lock+AnyOfOrNone(all) {
        action= LockMods(modifiers=Mod2);
    };
    interpret Scroll_Lock+AnyOfOrNone(all) {
        action= LockMods(modifiers=Mod3);
    };
    interpret Any+AnyOf(all) {
        action= SetMods(modifiers=modMapMods,clearLocks);
    };
    indicator "Caps Lock" {
        whichModState= locked;
        modifiers= Lock;
    };
    indicator "Num Lock" {
        whichModState= locked;
        modifiers= Mod2;
    };
    indicator "Scroll Lock" {
        whichModState= locked;
        modifiers= Mod3;
    };
};"#;

/// Emit a complete `xkb_keymap` for the given table.
#[must_use]
pub fn keymap_text(keys: &[KeyEntry], layout_name: &str) -> String {
    let mut s = String::with_capacity(16 * 1024);
    s.push_str("xkb_keymap {\n");

    // --- keycodes -------------------------------------------------------
    s.push_str("xkb_keycodes \"hairetsu\" {\n    minimum = 8;\n    maximum = 255;\n");
    for e in keys {
        let _ = writeln!(s, "    <{}> = {};", e.name, e.keycode);
    }
    for (i, name) in crate::LED_NAMES.iter().enumerate() {
        let _ = writeln!(s, "    indicator {} = \"{}\";", i + 1, name);
    }
    s.push_str("};\n\n");

    // --- types + compat -------------------------------------------------
    s.push_str(TYPES);
    s.push_str("\n\n");
    s.push_str(COMPAT);
    s.push_str("\n\n");

    // --- symbols --------------------------------------------------------
    let _ = writeln!(
        s,
        "xkb_symbols \"hairetsu\" {{\n    name[Group1]=\"{layout_name}\";"
    );
    for e in keys {
        let syms: Vec<String> = e.levels.iter().map(|r| keysym_name(*r)).collect();
        let _ = writeln!(
            s,
            "    key <{}> {{ type= \"{}\", symbols[Group1]= [ {} ] }};",
            e.name,
            e.kind.xkb_name(),
            syms.join(", ")
        );
    }
    // Group the modifier map so each modifier is declared once.
    for modname in [
        "Shift", "Lock", "Control", "Mod1", "Mod2", "Mod3", "Mod4", "Mod5",
    ] {
        let members: Vec<&str> = keys
            .iter()
            .filter(|e| modmap_entry(e) == Some(modname))
            .map(|e| e.name)
            .collect();
        if !members.is_empty() {
            let list = members
                .iter()
                .map(|n| format!("<{n}>"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(s, "    modifier_map {modname} {{ {list} }};");
        }
    }
    s.push_str("};\n");

    s.push_str("};\n");
    s
}

#[cfg(test)]
mod parity {
    //! ★ THE SEAM THE MODULE HEADER PROMISED.
    //!
    //! `TYPES` above encodes the level rules in XKB's vocabulary;
    //! `State::level_for_key` encodes the same rules in Rust. Nothing makes
    //! them agree — the header called that "the seam this module cannot make
    //! unrepresentable, and the parity test is what guards it", and until this
    //! module existed **there was no such test**. The tests below it are
    //! structural (sections present, braces balanced, names normalised); every
    //! one of them passes while the two sides disagree about what Shift+NumLock
    //! means.
    //!
    //! A disagreement here is invisible in the worst way. We resolve keysyms
    //! internally for our own bindings, and the CLIENT resolves them from the
    //! emitted text — so a mismatch means the compositor and the application
    //! believe different keys were pressed, with no error on either side.
    //!
    //! This parses the ACTUAL emitted text rather than restating the rules, so
    //! editing `TYPES` moves the expectation. Restating them in the test would
    //! only prove the test agrees with itself.

    use super::TYPES;
    use crate::layout::{KeyType, LAYOUTS};
    use crate::{Keymap, State, modifier};
    use std::collections::HashMap;
    use std::sync::Arc;

    /// `map[...]` sets, per XKB type name: masked modifiers -> 0-based level.
    struct XkbType {
        mask: u32,
        maps: HashMap<u32, u32>,
    }

    fn mod_bit(name: &str) -> u32 {
        match name.trim() {
            "none" | "None" => 0,
            n => 1 << modifier::index_of(n).unwrap_or_else(|| panic!("unknown modifier {n}")),
        }
    }

    fn mod_set(expr: &str) -> u32 {
        expr.split('+').map(mod_bit).fold(0, |a, b| a | b)
    }

    /// Parse the `xkb_types` block into a table keyed by XKB type name.
    fn parse_types() -> HashMap<String, XkbType> {
        let mut out = HashMap::new();
        let mut current: Option<(String, XkbType)> = None;
        for line in TYPES.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("type \"") {
                let name = rest.split('"').next().expect("type name").to_owned();
                current = Some((
                    name,
                    XkbType {
                        mask: 0,
                        maps: HashMap::new(),
                    },
                ));
            } else if let Some(rest) = t.strip_prefix("modifiers=") {
                if let Some((_, ty)) = current.as_mut() {
                    ty.mask = mod_set(rest.trim_end_matches(';'));
                }
            } else if let Some(rest) = t.strip_prefix("map[") {
                if let Some((_, ty)) = current.as_mut() {
                    let (mods, lvl) = rest.split_once("]=").expect("map entry shape");
                    let level: u32 = lvl
                        .trim()
                        .trim_end_matches(';')
                        .trim_start_matches("Level")
                        .parse()
                        .expect("level number");
                    ty.maps.insert(mod_set(mods), level - 1);
                }
            } else if t == "};"
                && let Some((name, ty)) = current.take()
            {
                out.insert(name, ty);
            }
        }
        out
    }

    /// The level a CLIENT would pick, given the emitted type and live mods.
    fn client_level(ty: &XkbType, mods: u32) -> u32 {
        *ty.maps.get(&(mods & ty.mask)).unwrap_or(&0)
    }

    #[test]
    fn the_emitted_types_cover_every_key_type_we_can_declare() {
        // Anti-vacuity: a key whose type is absent from `TYPES` emits a keymap
        // that names an undeclared type, which clients reject outright — and
        // the loop below would silently skip it.
        let parsed = parse_types();
        for kind in [
            KeyType::OneLevel,
            KeyType::TwoLevel,
            KeyType::Alphabetic,
            KeyType::Keypad,
            KeyType::FourLevel,
            KeyType::FourLevelAlphabetic,
            KeyType::AltLevel2,
            KeyType::ControlLevel2,
        ] {
            assert!(
                parsed.contains_key(kind.xkb_name()),
                "{:?} emits type \"{}\", which TYPES does not declare",
                kind,
                kind.xkb_name()
            );
        }
    }

    #[test]
    fn our_resolver_and_the_emitted_types_agree_on_every_key() {
        let parsed = parse_types();
        // Every modifier combination the emitted types can distinguish.
        let combos = [
            0,
            modifier::SHIFT,
            modifier::LOCK,
            modifier::SHIFT | modifier::LOCK,
            modifier::MOD2,
            modifier::SHIFT | modifier::MOD2,
            modifier::MOD1,
            modifier::SHIFT | modifier::MOD1,
            modifier::CONTROL,
            modifier::SHIFT | modifier::CONTROL,
            modifier::MOD5,
            modifier::SHIFT | modifier::MOD5,
            modifier::LOCK | modifier::MOD5,
            modifier::SHIFT | modifier::LOCK | modifier::MOD5,
        ];

        let mut checked = 0_u32;
        for def in LAYOUTS {
            let keymap = Arc::new(Keymap::for_layout(def.rmlvo).expect("registered layout"));
            for entry in def.keys {
                let ty = parsed
                    .get(entry.kind.xkb_name())
                    .expect("covered by the test above");
                for mods in combos {
                    let mut st = State::new(Arc::clone(&keymap));
                    // Lock is a LOCKED modifier; the rest are depressed.
                    st.update_mask(mods & !modifier::LOCK, 0, mods & modifier::LOCK, 0, 0, 0);

                    let ours = st.level_for_key(entry.keycode);
                    let theirs = client_level(ty, mods)
                        .min(u32::try_from(entry.levels.len().saturating_sub(1)).expect("small"));
                    assert_eq!(
                        ours,
                        theirs,
                        "{}/{} ({:?}) under mods {mods:#06b}: we resolve level {ours}, a client \
                         reading the emitted \"{}\" type resolves level {theirs}",
                        def.rmlvo,
                        entry.name,
                        entry.kind,
                        entry.kind.xkb_name()
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 1000,
            "only {checked} (key, modifier) pairs compared — the loop is not \
             reaching the tables it is supposed to cover"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::US;

    fn text() -> String {
        keymap_text(US, "English (US)")
    }

    #[test]
    fn has_all_four_required_sections() {
        // A keymap missing any of these is rejected wholesale by clients.
        let t = text();
        for section in [
            "xkb_keycodes",
            "xkb_types",
            "xkb_compatibility",
            "xkb_symbols",
        ] {
            assert!(t.contains(section), "missing {section}");
        }
        assert!(t.starts_with("xkb_keymap {"));
        // The outer block closes with `};` — XKB terminates every section,
        // including the keymap itself, with a semicolon.
        assert!(t.trim_end().ends_with("};"));
    }

    #[test]
    fn braces_balance() {
        let t = text();
        let open = t.matches('{').count();
        let close = t.matches('}').count();
        assert_eq!(open, close, "unbalanced braces — client compile would fail");
    }

    #[test]
    fn never_emits_an_include() {
        // An include would reintroduce the xkeyboard-config data dependency
        // this crate exists to remove.
        assert!(!text().contains("include"));
    }

    #[test]
    fn declares_no_virtual_modifiers() {
        // See the module docs: an unbound virtual modifier fails silently.
        assert!(!text().contains("virtual_modifiers"));
    }

    #[test]
    fn every_key_in_the_table_is_emitted() {
        let t = text();
        for e in US {
            assert!(
                t.contains(&format!("<{}> = {};", e.name, e.keycode)),
                "{}",
                e.name
            );
            assert!(
                t.contains(&format!("key <{}> {{", e.name)),
                "{} symbols",
                e.name
            );
        }
    }

    #[test]
    fn keysym_names_are_stripped_to_xkb_spelling() {
        // The exact trap the normaliser exists for: xkeysym hands back C header
        // macro names. Any `XK_` reaching the output is a keymap clients reject.
        let t = text();
        assert!(t.contains("XF86AudioMute"), "vendor name not normalised");
        assert!(t.contains("[ a, A ]"), "plain letters not normalised");
        assert!(t.contains("KP_Home"), "keypad name not normalised");
        assert!(
            !t.contains("XK_"),
            "emitted a C macro name, not XKB's spelling"
        );
    }

    #[test]
    fn modifier_map_binds_both_shifts() {
        let t = text();
        let line = t
            .lines()
            .find(|l| l.contains("modifier_map Shift"))
            .expect("Shift modifier_map present");
        assert!(line.contains("<LFSH>"));
        assert!(line.contains("<RTSH>"));
    }

    #[test]
    fn alphabetic_keys_declare_the_alphabetic_type() {
        let t = text();
        assert!(t.contains(r#"key <AC01> { type= "ALPHABETIC""#));
        // and digits must not
        assert!(t.contains(r#"key <AE01> { type= "TWO_LEVEL""#));
    }

    #[test]
    fn unicode_keysyms_use_xkbs_u_spelling_not_a_raw_number() {
        // ★ REGRESSION, from the same `xkbcli` differential. `xkeysym` has no
        // name for BULLET or LATIN CAPITAL SHARP S, and the numeric fallback
        // emitted `0x01002022`. XKB accepts it, so nothing broke — but a wire
        // format is read by people too, and `U2022` says BULLET while
        // `0x01002022` says nothing until you subtract the prefix.
        let t = keymap_text(crate::layout::BR, "Portuguese (Brazil)");
        assert!(t.contains("U2022"), "BULLET not in U-form");
        assert!(t.contains("U1E9E"), "LATIN CAPITAL SHARP S not in U-form");
        assert!(
            !t.contains("0x0100"),
            "a Unicode keysym is still emitting the raw numeric form"
        );
    }

    #[test]
    fn keysym_names_are_bare_identifiers() {
        // A `key::` prefix leaking through would be a compile error client-side.
        assert!(!text().contains("key::"));
    }
}
