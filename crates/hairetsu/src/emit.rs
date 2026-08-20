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
    // No name: fall back to the numeric form XKB always accepts.
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
        map[Shift]= Level2;
        map[Mod2]= Level2;
        level_name[Level1]= "Base";
        level_name[Level2]= "Number";
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
    let _ = writeln!(s, "xkb_symbols \"hairetsu\" {{\n    name[Group1]=\"{layout_name}\";");
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
    for modname in ["Shift", "Lock", "Control", "Mod1", "Mod2", "Mod3", "Mod4", "Mod5"] {
        let members: Vec<&str> = keys
            .iter()
            .filter(|e| modmap_entry(e) == Some(modname))
            .map(|e| e.name)
            .collect();
        if !members.is_empty() {
            let list = members.iter().map(|n| format!("<{n}>")).collect::<Vec<_>>().join(", ");
            let _ = writeln!(s, "    modifier_map {modname} {{ {list} }};");
        }
    }
    s.push_str("};\n");

    s.push_str("};\n");
    s
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
        for section in ["xkb_keycodes", "xkb_types", "xkb_compatibility", "xkb_symbols"] {
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
            assert!(t.contains(&format!("<{}> = {};", e.name, e.keycode)), "{}", e.name);
            assert!(t.contains(&format!("key <{}> {{", e.name)), "{} symbols", e.name);
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
        assert!(!t.contains("XK_"), "emitted a C macro name, not XKB's spelling");
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
    fn keysym_names_are_bare_identifiers() {
        // A `key::` prefix leaking through would be a compile error client-side.
        assert!(!text().contains("key::"));
    }
}
