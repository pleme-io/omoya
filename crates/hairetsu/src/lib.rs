//! hairetsu (配列) — a pure-Rust keyboard layout engine.
//!
//! Owns the three things a compositor needs from a keyboard:
//!
//! 1. **resolution** — which keysym a keycode produces under the current modifiers,
//! 2. **modifier state** — what Shift/Caps/Ctrl/Alt/Super are doing right now,
//! 3. **emission** — the XKB keymap *text* handed to Wayland clients.
//!
//! All three read the SAME table ([`layout::US`]), so the keymap we resolve
//! against and the keymap clients compile cannot drift apart. That is the one
//! structural guarantee here; everything else is ordinary code.
//!
//! # Scope, stated plainly
//!
//! This is not an XKB implementation. It does not read `/usr/share/X11/xkb`,
//! does not parse keymap text, and ships exactly one layout. It covers the
//! single-layout `us` seat and says so rather than implying more.

#![allow(clippy::module_name_repetitions)]

pub mod emit;
pub mod layout;

use std::sync::Arc;

pub use xkeysym::{Keysym, RawKeysym};

/// A modifier mask — the same bit order X11 and XKB use.
pub type ModMask = u32;

/// Modifier bits, in XKB's canonical order.
pub mod modifier {
    use super::ModMask;

    pub const SHIFT: ModMask = 1 << 0;
    /// Caps Lock. XKB calls this "Lock", not "Caps".
    pub const LOCK: ModMask = 1 << 1;
    pub const CONTROL: ModMask = 1 << 2;
    /// Alt.
    pub const MOD1: ModMask = 1 << 3;
    /// Num Lock.
    pub const MOD2: ModMask = 1 << 4;
    pub const MOD3: ModMask = 1 << 5;
    /// Super / Logo.
    pub const MOD4: ModMask = 1 << 6;
    /// `AltGr` / `ISO_Level3_Shift`.
    pub const MOD5: ModMask = 1 << 7;

    /// The canonical XKB modifier names, in bit order.
    ///
    /// Index is the modifier index; position in this array IS the bit.
    pub const NAMES: [&str; 8] = [
        "Shift", "Lock", "Control", "Mod1", "Mod2", "Mod3", "Mod4", "Mod5",
    ];

    /// Resolve a modifier name to its index, matching `xkb_keymap_mod_get_index`.
    #[must_use]
    pub fn index_of(name: &str) -> Option<u32> {
        NAMES
            .iter()
            .position(|n| *n == name)
            .map(|i| u32::try_from(i).expect("8 names fit in u32"))
    }
}

/// LED names, in index order.
pub const LED_NAMES: [&str; 3] = ["Caps Lock", "Num Lock", "Scroll Lock"];

/// Which parts of the state a [`State::update_key`] call changed.
///
/// Same bit layout as `xkb_state_component`, because these values cross into
/// smithay and out to Wayland clients unchanged.
pub mod component {
    pub const MODS_DEPRESSED: u32 = 1 << 0;
    pub const MODS_LATCHED: u32 = 1 << 1;
    pub const MODS_LOCKED: u32 = 1 << 2;
    pub const MODS_EFFECTIVE: u32 = 1 << 3;
    pub const LAYOUT_DEPRESSED: u32 = 1 << 4;
    pub const LAYOUT_LATCHED: u32 = 1 << 5;
    pub const LAYOUT_LOCKED: u32 = 1 << 6;
    pub const LAYOUT_EFFECTIVE: u32 = 1 << 7;
    pub const LEDS: u32 = 1 << 8;
}

/// Whether a key went down or came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDirection {
    Up,
    Down,
}

/// One key, compiled: keysyms materialised so a borrowed slice can be handed out.
#[derive(Debug, Clone)]
struct CompiledKey {
    keycode: u32,
    name: &'static str,
    kind: layout::KeyType,
    levels: Vec<Keysym>,
}

/// A compiled keymap.
#[derive(Debug)]
pub struct Keymap {
    keys: Vec<CompiledKey>,
    layout_name: String,
    text: String,
}

impl Keymap {
    /// Compile the built-in `us` layout.
    ///
    /// Never fails — the table is compiled in, so there is no I/O to go wrong.
    /// That is the whole reason this returns `Self` and not `Option<Self>`.
    #[must_use]
    pub fn us() -> Self {
        let keys: Vec<CompiledKey> = layout::US
            .iter()
            .map(|e| CompiledKey {
                keycode: e.keycode,
                name: e.name,
                kind: e.kind,
                levels: e.levels.iter().copied().map(Keysym::new).collect(),
            })
            .collect();
        let text = emit::keymap_text(layout::US, "English (US)");
        Self { keys, layout_name: "English (US)".to_owned(), text }
    }

    fn key(&self, keycode: u32) -> Option<&CompiledKey> {
        self.keys.binary_search_by_key(&keycode, |k| k.keycode).ok().map(|i| &self.keys[i])
    }

    /// The keymap as XKB text — what gets sent to Wayland clients.
    #[must_use]
    pub fn as_text(&self) -> &str {
        &self.text
    }

    /// Number of layouts. Always 1 here; see the crate-level scope note.
    #[must_use]
    pub const fn num_layouts(&self) -> u32 {
        1
    }

    #[must_use]
    pub fn layout_name(&self, _idx: u32) -> &str {
        &self.layout_name
    }

    #[must_use]
    pub fn num_levels_for_key(&self, keycode: u32) -> u32 {
        self.key(keycode)
            .map_or(0, |k| u32::try_from(k.levels.len()).expect("level count is small"))
    }

    #[must_use]
    pub fn min_keycode(&self) -> u32 {
        self.keys.first().map_or(8, |k| k.keycode)
    }

    #[must_use]
    pub fn max_keycode(&self) -> u32 {
        self.keys.last().map_or(255, |k| k.keycode)
    }

    /// Keysyms for an explicit level, bypassing modifier state.
    ///
    /// An out-of-range level is clamped rather than rejected, matching
    /// `xkb_keymap_key_get_syms_by_level`.
    #[must_use]
    pub fn key_syms_by_level(&self, keycode: u32, level: u32) -> &[Keysym] {
        let Some(k) = self.key(keycode) else { return &[] };
        if k.levels.is_empty() {
            return &[];
        }
        let idx = (level as usize).min(k.levels.len() - 1);
        &k.levels[idx..=idx]
    }

    /// Does this key repeat when held?
    ///
    /// Modifier and lock keys must not, or holding Shift types a stream of them.
    #[must_use]
    pub fn key_repeats(&self, keycode: u32) -> bool {
        self.key(keycode).is_some_and(|k| {
            k.levels.first().is_none_or(|s| !is_modifier_keysym(*s))
        })
    }
}

/// Is this keysym a modifier or lock key?
fn is_modifier_keysym(sym: Keysym) -> bool {
    use xkeysym::key;
    matches!(
        sym.raw(),
        key::Shift_L
            | key::Shift_R
            | key::Control_L
            | key::Control_R
            | key::Alt_L
            | key::Alt_R
            | key::Super_L
            | key::Super_R
            | key::Meta_L
            | key::Meta_R
            | key::Caps_Lock
            | key::Num_Lock
            | key::Scroll_Lock
            | key::ISO_Level3_Shift
    )
}

/// Which modifier bit a keysym contributes while held, if any.
fn modifier_bit(sym: Keysym) -> Option<ModMask> {
    use xkeysym::key;
    Some(match sym.raw() {
        key::Shift_L | key::Shift_R => modifier::SHIFT,
        key::Control_L | key::Control_R => modifier::CONTROL,
        key::Alt_L | key::Meta_L | key::Meta_R => modifier::MOD1,
        key::Super_L | key::Super_R => modifier::MOD4,
        key::Alt_R | key::ISO_Level3_Shift => modifier::MOD5,
        _ => return None,
    })
}

/// Which modifier a keysym *locks* when pressed, if any.
fn lock_bit(sym: Keysym) -> Option<ModMask> {
    use xkeysym::key;
    Some(match sym.raw() {
        key::Caps_Lock => modifier::LOCK,
        key::Num_Lock => modifier::MOD2,
        _ => return None,
    })
}

/// Live keyboard state over a [`Keymap`].
#[derive(Debug)]
pub struct State {
    keymap: Arc<Keymap>,
    depressed: ModMask,
    latched: ModMask,
    locked: ModMask,
    layout_depressed: u32,
    layout_latched: u32,
    layout_locked: u32,
    /// Keycodes currently held. Needed because two Shift keys can be down at
    /// once — releasing one must not clear the modifier while the other is held.
    held: Vec<u32>,
}

impl State {
    #[must_use]
    pub fn new(keymap: Arc<Keymap>) -> Self {
        Self {
            keymap,
            depressed: 0,
            latched: 0,
            locked: 0,
            layout_depressed: 0,
            layout_latched: 0,
            layout_locked: 0,
            held: Vec::new(),
        }
    }

    #[must_use]
    pub fn keymap(&self) -> &Arc<Keymap> {
        &self.keymap
    }

    /// The modifiers in effect right now.
    #[must_use]
    pub const fn effective_mods(&self) -> ModMask {
        self.depressed | self.latched | self.locked
    }

    /// Feed a key event. Returns which state components changed.
    pub fn update_key(&mut self, keycode: u32, direction: KeyDirection) -> u32 {
        let before = (self.depressed, self.latched, self.locked);

        let sym = self
            .keymap
            .key(keycode)
            .and_then(|k| k.levels.first().copied());

        match direction {
            KeyDirection::Down => {
                if !self.held.contains(&keycode) {
                    self.held.push(keycode);
                }
                if let Some(s) = sym {
                    if let Some(bit) = modifier_bit(s) {
                        self.depressed |= bit;
                    }
                    // A lock toggles on press only. Toggling on release too
                    // would make Caps Lock a no-op for every complete press.
                    if let Some(bit) = lock_bit(s) {
                        self.locked ^= bit;
                    }
                }
            }
            KeyDirection::Up => {
                self.held.retain(|k| *k != keycode);
                // Recompute from what is still held rather than clearing the
                // bit: with both Shifts down, releasing one must keep Shift.
                self.depressed = 0;
                for held in &self.held {
                    if let Some(s) = self.keymap.key(*held).and_then(|k| k.levels.first().copied())
                    {
                        if let Some(bit) = modifier_bit(s) {
                            self.depressed |= bit;
                        }
                    }
                }
            }
        }

        let mut changed = 0;
        if before.0 != self.depressed {
            changed |= component::MODS_DEPRESSED;
        }
        if before.1 != self.latched {
            changed |= component::MODS_LATCHED;
        }
        if before.2 != self.locked {
            changed |= component::MODS_LOCKED;
        }
        if changed != 0 {
            changed |= component::MODS_EFFECTIVE;
        }
        if before.2 != self.locked {
            changed |= component::LEDS;
        }
        changed
    }

    /// Adopt modifier state computed elsewhere (a client, or a seat handoff).
    pub fn update_mask(
        &mut self,
        depressed: ModMask,
        latched: ModMask,
        locked: ModMask,
        layout_depressed: u32,
        layout_latched: u32,
        layout_locked: u32,
    ) -> u32 {
        let before = (self.depressed, self.latched, self.locked);
        self.depressed = depressed;
        self.latched = latched;
        self.locked = locked;
        self.layout_depressed = layout_depressed;
        self.layout_latched = layout_latched;
        self.layout_locked = layout_locked;

        let mut changed = 0;
        if before.0 != self.depressed {
            changed |= component::MODS_DEPRESSED;
        }
        if before.1 != self.latched {
            changed |= component::MODS_LATCHED;
        }
        if before.2 != self.locked {
            changed |= component::MODS_LOCKED | component::LEDS;
        }
        if changed != 0 {
            changed |= component::MODS_EFFECTIVE;
        }
        changed
    }

    #[must_use]
    pub const fn serialize_mods(&self, components: u32) -> ModMask {
        let mut out = 0;
        if components & component::MODS_DEPRESSED != 0 {
            out |= self.depressed;
        }
        if components & component::MODS_LATCHED != 0 {
            out |= self.latched;
        }
        if components & component::MODS_LOCKED != 0 {
            out |= self.locked;
        }
        if components & component::MODS_EFFECTIVE != 0 {
            out |= self.depressed | self.latched | self.locked;
        }
        out
    }

    #[must_use]
    pub const fn serialize_layout(&self, components: u32) -> u32 {
        let mut out = 0;
        if components & component::LAYOUT_DEPRESSED != 0 {
            out |= self.layout_depressed;
        }
        if components & component::LAYOUT_LATCHED != 0 {
            out |= self.layout_latched;
        }
        if components & (component::LAYOUT_LOCKED | component::LAYOUT_EFFECTIVE) != 0 {
            out |= self.layout_locked;
        }
        out
    }

    #[must_use]
    pub fn mod_name_is_active(&self, name: &str) -> bool {
        modifier::index_of(name).is_some_and(|i| self.effective_mods() & (1 << i) != 0)
    }

    /// The layout index in effect. Always 0 — single-layout by scope.
    #[must_use]
    pub const fn layout_for_key(&self, _keycode: u32) -> u32 {
        0
    }

    /// Which shift level a key resolves to under the current modifiers.
    ///
    /// This is where the key *type* earns its keep: Caps Lock reaches letters
    /// and nothing else, Num Lock reaches the keypad and nothing else.
    #[must_use]
    pub fn level_for_key(&self, keycode: u32) -> u32 {
        let Some(k) = self.keymap.key(keycode) else { return 0 };
        let mods = self.effective_mods();
        let shift = mods & modifier::SHIFT != 0;
        let caps = mods & modifier::LOCK != 0;
        let num = mods & modifier::MOD2 != 0;
        let level3 = mods & modifier::MOD5 != 0;

        let base = match k.kind {
            layout::KeyType::OneLevel => 0,
            layout::KeyType::TwoLevel => u32::from(shift),
            // XOR, not OR: Shift on a capsed keyboard gives lowercase.
            layout::KeyType::Alphabetic => u32::from(shift ^ caps),
            layout::KeyType::Keypad => u32::from(shift || num),
        };
        // Level 3/4 only exist on keys that declare them.
        if level3 && k.levels.len() >= 4 { base + 2 } else { base }
    }

    /// Keysyms produced by a key right now.
    #[must_use]
    pub fn key_syms(&self, keycode: u32) -> &[Keysym] {
        self.keymap.key_syms_by_level(keycode, self.level_for_key(keycode))
    }

    /// The single keysym a key produces, or `NoSymbol`.
    #[must_use]
    pub fn key_sym(&self, keycode: u32) -> Keysym {
        self.key_syms(keycode).first().copied().unwrap_or(Keysym::new(0))
    }

    /// Active LED mask, bit order matching [`LED_NAMES`].
    #[must_use]
    pub const fn leds(&self) -> u32 {
        let mut out = 0;
        if self.locked & modifier::LOCK != 0 {
            out |= 1 << 0;
        }
        if self.locked & modifier::MOD2 != 0 {
            out |= 1 << 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xkeysym::key;

    fn state() -> State {
        State::new(Arc::new(Keymap::us()))
    }

    const A: u32 = 38;
    const ONE: u32 = 10;
    const LSHIFT: u32 = 50;
    const RSHIFT: u32 = 62;
    const CAPS: u32 = 66;
    const KP7: u32 = 79;
    const NUMLK: u32 = 77;

    #[test]
    fn plain_a_is_lowercase() {
        let s = state();
        assert_eq!(s.key_sym(A).raw(), key::a);
    }

    #[test]
    fn shift_a_is_uppercase() {
        let mut s = state();
        s.update_key(LSHIFT, KeyDirection::Down);
        assert_eq!(s.key_sym(A).raw(), key::A);
    }

    #[test]
    fn releasing_one_shift_while_the_other_is_held_keeps_shift() {
        // The reason `held` exists. Clearing the bit on any release would
        // drop Shift here, and the bug only shows with two hands on the keyboard.
        let mut s = state();
        s.update_key(LSHIFT, KeyDirection::Down);
        s.update_key(RSHIFT, KeyDirection::Down);
        s.update_key(LSHIFT, KeyDirection::Up);
        assert_eq!(s.key_sym(A).raw(), key::A, "Shift lost while RSHIFT still held");
        s.update_key(RSHIFT, KeyDirection::Up);
        assert_eq!(s.key_sym(A).raw(), key::a);
    }

    #[test]
    fn caps_lock_toggles_on_press_and_survives_release() {
        let mut s = state();
        s.update_key(CAPS, KeyDirection::Down);
        s.update_key(CAPS, KeyDirection::Up);
        assert_eq!(s.key_sym(A).raw(), key::A, "caps did not latch");
        s.update_key(CAPS, KeyDirection::Down);
        s.update_key(CAPS, KeyDirection::Up);
        assert_eq!(s.key_sym(A).raw(), key::a, "caps did not release");
    }

    #[test]
    fn shift_on_a_capsed_keyboard_gives_lowercase() {
        // The XOR rule. An OR would give uppercase and feel subtly broken.
        let mut s = state();
        s.update_key(CAPS, KeyDirection::Down);
        s.update_key(CAPS, KeyDirection::Up);
        s.update_key(LSHIFT, KeyDirection::Down);
        assert_eq!(s.key_sym(A).raw(), key::a);
    }

    #[test]
    fn caps_lock_does_not_reach_digits() {
        let mut s = state();
        s.update_key(CAPS, KeyDirection::Down);
        s.update_key(CAPS, KeyDirection::Up);
        assert_eq!(s.key_sym(ONE).raw(), key::_1, "caps turned 1 into !");
        s.update_key(LSHIFT, KeyDirection::Down);
        assert_eq!(s.key_sym(ONE).raw(), key::exclam);
    }

    #[test]
    fn num_lock_reaches_the_keypad_only() {
        let mut s = state();
        assert_eq!(s.key_sym(KP7).raw(), key::KP_Home);
        s.update_key(NUMLK, KeyDirection::Down);
        s.update_key(NUMLK, KeyDirection::Up);
        assert_eq!(s.key_sym(KP7).raw(), key::KP_7);
        assert_eq!(s.key_sym(A).raw(), key::a, "num lock reached a letter");
    }

    #[test]
    fn ctrl_is_reported_by_name() {
        let mut s = state();
        s.update_key(37, KeyDirection::Down);
        assert!(s.mod_name_is_active("Control"));
        assert!(!s.mod_name_is_active("Shift"));
    }

    #[test]
    fn update_key_reports_what_changed() {
        let mut s = state();
        let c = s.update_key(LSHIFT, KeyDirection::Down);
        assert!(c & component::MODS_DEPRESSED != 0);
        assert!(c & component::MODS_EFFECTIVE != 0);
        // A plain letter moves nothing.
        assert_eq!(s.update_key(A, KeyDirection::Down), 0);
    }

    #[test]
    fn caps_lock_lights_its_led() {
        let mut s = state();
        assert_eq!(s.leds(), 0);
        let c = s.update_key(CAPS, KeyDirection::Down);
        assert!(c & component::LEDS != 0);
        assert_eq!(s.leds() & 1, 1);
    }

    #[test]
    fn modifier_keys_do_not_repeat() {
        let km = Keymap::us();
        assert!(!km.key_repeats(LSHIFT));
        assert!(!km.key_repeats(CAPS));
        assert!(km.key_repeats(A));
    }

    #[test]
    fn unknown_keycodes_are_silent_not_panics() {
        let mut s = state();
        assert_eq!(s.update_key(9999, KeyDirection::Down), 0);
        assert!(s.key_syms(9999).is_empty());
        assert_eq!(s.key_sym(9999).raw(), 0);
    }

    #[test]
    fn mod_indices_match_xkb_canonical_order() {
        // These cross the wire to clients; a wrong index is a wrong keyboard.
        assert_eq!(modifier::index_of("Shift"), Some(0));
        assert_eq!(modifier::index_of("Lock"), Some(1));
        assert_eq!(modifier::index_of("Control"), Some(2));
        assert_eq!(modifier::index_of("Mod1"), Some(3));
        assert_eq!(modifier::index_of("Mod4"), Some(6));
        assert_eq!(modifier::index_of("NoSuchMod"), None);
    }

    #[test]
    fn serialize_mods_selects_by_component() {
        let mut s = state();
        s.update_key(LSHIFT, KeyDirection::Down);
        assert_eq!(s.serialize_mods(component::MODS_DEPRESSED), modifier::SHIFT);
        assert_eq!(s.serialize_mods(component::MODS_LOCKED), 0);
        assert_eq!(s.serialize_mods(component::MODS_EFFECTIVE), modifier::SHIFT);
    }
}
