//! The keycode → keysym table.
//!
//! XKB keycode = evdev code + 8. Each entry carries the XKB key *name* as well,
//! because the same table emits the keymap text handed to Wayland clients — one
//! source, so the table we resolve against and the table clients compile can
//! never disagree.

use xkeysym::key;
use xkeysym::RawKeysym;

/// How a key selects its shift level.
///
/// This is XKB's key *type*. It is what makes Caps Lock affect `a` but not `1`,
/// and Num Lock affect the keypad but nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// One level, modifiers ignored (Escape, Return, the modifier keys).
    OneLevel,
    /// Shift selects level 2. Caps Lock does NOT apply.
    TwoLevel,
    /// Shift XOR Caps selects level 2 — letters.
    Alphabetic,
    /// Shift or Num Lock selects level 2 — the keypad.
    Keypad,
}

impl KeyType {
    /// The XKB type name used in emitted keymap text.
    #[must_use]
    pub const fn xkb_name(self) -> &'static str {
        match self {
            Self::OneLevel => "ONE_LEVEL",
            Self::TwoLevel => "TWO_LEVEL",
            Self::Alphabetic => "ALPHABETIC",
            Self::Keypad => "KEYPAD",
        }
    }
}

/// One physical key.
#[derive(Debug, Clone, Copy)]
pub struct KeyEntry {
    /// XKB keycode (evdev + 8).
    pub keycode: u32,
    /// XKB key name, e.g. `AC01`. Used only for keymap-text emission.
    pub name: &'static str,
    pub kind: KeyType,
    /// Keysyms by level. Always 1, 2 or 4 entries.
    pub levels: &'static [RawKeysym],
}

const fn k(
    keycode: u32,
    name: &'static str,
    kind: KeyType,
    levels: &'static [RawKeysym],
) -> KeyEntry {
    KeyEntry { keycode, name, kind, levels }
}

use KeyType::{Alphabetic, Keypad, OneLevel, TwoLevel};

/// The `us` layout.
///
/// Ordered by keycode so lookup is a binary search.
pub static US: &[KeyEntry] = &[
    k(9, "ESC", OneLevel, &[key::Escape]),
    k(10, "AE01", TwoLevel, &[key::_1, key::exclam]),
    k(11, "AE02", TwoLevel, &[key::_2, key::at]),
    k(12, "AE03", TwoLevel, &[key::_3, key::numbersign]),
    k(13, "AE04", TwoLevel, &[key::_4, key::dollar]),
    k(14, "AE05", TwoLevel, &[key::_5, key::percent]),
    k(15, "AE06", TwoLevel, &[key::_6, key::asciicircum]),
    k(16, "AE07", TwoLevel, &[key::_7, key::ampersand]),
    k(17, "AE08", TwoLevel, &[key::_8, key::asterisk]),
    k(18, "AE09", TwoLevel, &[key::_9, key::parenleft]),
    k(19, "AE10", TwoLevel, &[key::_0, key::parenright]),
    k(20, "AE11", TwoLevel, &[key::minus, key::underscore]),
    k(21, "AE12", TwoLevel, &[key::equal, key::plus]),
    k(22, "BKSP", OneLevel, &[key::BackSpace]),
    k(23, "TAB", OneLevel, &[key::Tab]),
    k(24, "AD01", Alphabetic, &[key::q, key::Q]),
    k(25, "AD02", Alphabetic, &[key::w, key::W]),
    k(26, "AD03", Alphabetic, &[key::e, key::E]),
    k(27, "AD04", Alphabetic, &[key::r, key::R]),
    k(28, "AD05", Alphabetic, &[key::t, key::T]),
    k(29, "AD06", Alphabetic, &[key::y, key::Y]),
    k(30, "AD07", Alphabetic, &[key::u, key::U]),
    k(31, "AD08", Alphabetic, &[key::i, key::I]),
    k(32, "AD09", Alphabetic, &[key::o, key::O]),
    k(33, "AD10", Alphabetic, &[key::p, key::P]),
    k(34, "AD11", TwoLevel, &[key::bracketleft, key::braceleft]),
    k(35, "AD12", TwoLevel, &[key::bracketright, key::braceright]),
    k(36, "RTRN", OneLevel, &[key::Return]),
    k(37, "LCTL", OneLevel, &[key::Control_L]),
    k(38, "AC01", Alphabetic, &[key::a, key::A]),
    k(39, "AC02", Alphabetic, &[key::s, key::S]),
    k(40, "AC03", Alphabetic, &[key::d, key::D]),
    k(41, "AC04", Alphabetic, &[key::f, key::F]),
    k(42, "AC05", Alphabetic, &[key::g, key::G]),
    k(43, "AC06", Alphabetic, &[key::h, key::H]),
    k(44, "AC07", Alphabetic, &[key::j, key::J]),
    k(45, "AC08", Alphabetic, &[key::k, key::K]),
    k(46, "AC09", Alphabetic, &[key::l, key::L]),
    k(47, "AC10", TwoLevel, &[key::semicolon, key::colon]),
    k(48, "AC11", TwoLevel, &[key::apostrophe, key::quotedbl]),
    k(49, "TLDE", TwoLevel, &[key::grave, key::asciitilde]),
    k(50, "LFSH", OneLevel, &[key::Shift_L]),
    k(51, "BKSL", TwoLevel, &[key::backslash, key::bar]),
    k(52, "AB01", Alphabetic, &[key::z, key::Z]),
    k(53, "AB02", Alphabetic, &[key::x, key::X]),
    k(54, "AB03", Alphabetic, &[key::c, key::C]),
    k(55, "AB04", Alphabetic, &[key::v, key::V]),
    k(56, "AB05", Alphabetic, &[key::b, key::B]),
    k(57, "AB06", Alphabetic, &[key::n, key::N]),
    k(58, "AB07", Alphabetic, &[key::m, key::M]),
    k(59, "AB08", TwoLevel, &[key::comma, key::less]),
    k(60, "AB09", TwoLevel, &[key::period, key::greater]),
    k(61, "AB10", TwoLevel, &[key::slash, key::question]),
    k(62, "RTSH", OneLevel, &[key::Shift_R]),
    k(63, "KPMU", OneLevel, &[key::KP_Multiply]),
    k(64, "LALT", OneLevel, &[key::Alt_L]),
    k(65, "SPCE", OneLevel, &[key::space]),
    k(66, "CAPS", OneLevel, &[key::Caps_Lock]),
    k(67, "FK01", OneLevel, &[key::F1]),
    k(68, "FK02", OneLevel, &[key::F2]),
    k(69, "FK03", OneLevel, &[key::F3]),
    k(70, "FK04", OneLevel, &[key::F4]),
    k(71, "FK05", OneLevel, &[key::F5]),
    k(72, "FK06", OneLevel, &[key::F6]),
    k(73, "FK07", OneLevel, &[key::F7]),
    k(74, "FK08", OneLevel, &[key::F8]),
    k(75, "FK09", OneLevel, &[key::F9]),
    k(76, "FK10", OneLevel, &[key::F10]),
    k(77, "NMLK", OneLevel, &[key::Num_Lock]),
    k(78, "SCLK", OneLevel, &[key::Scroll_Lock]),
    k(79, "KP7", Keypad, &[key::KP_Home, key::KP_7]),
    k(80, "KP8", Keypad, &[key::KP_Up, key::KP_8]),
    k(81, "KP9", Keypad, &[key::KP_Prior, key::KP_9]),
    k(82, "KPSU", OneLevel, &[key::KP_Subtract]),
    k(83, "KP4", Keypad, &[key::KP_Left, key::KP_4]),
    k(84, "KP5", Keypad, &[key::KP_Begin, key::KP_5]),
    k(85, "KP6", Keypad, &[key::KP_Right, key::KP_6]),
    k(86, "KPAD", OneLevel, &[key::KP_Add]),
    k(87, "KP1", Keypad, &[key::KP_End, key::KP_1]),
    k(88, "KP2", Keypad, &[key::KP_Down, key::KP_2]),
    k(89, "KP3", Keypad, &[key::KP_Next, key::KP_3]),
    k(90, "KP0", Keypad, &[key::KP_Insert, key::KP_0]),
    k(91, "KPDL", Keypad, &[key::KP_Delete, key::KP_Decimal]),
    k(94, "LSGT", TwoLevel, &[key::less, key::greater]),
    k(95, "FK11", OneLevel, &[key::F11]),
    k(96, "FK12", OneLevel, &[key::F12]),
    k(104, "KPEN", OneLevel, &[key::KP_Enter]),
    k(105, "RCTL", OneLevel, &[key::Control_R]),
    k(106, "KPDV", OneLevel, &[key::KP_Divide]),
    k(107, "PRSC", OneLevel, &[key::Print]),
    k(108, "RALT", OneLevel, &[key::Alt_R]),
    k(110, "HOME", OneLevel, &[key::Home]),
    k(111, "UP", OneLevel, &[key::Up]),
    k(112, "PGUP", OneLevel, &[key::Prior]),
    k(113, "LEFT", OneLevel, &[key::Left]),
    k(114, "RGHT", OneLevel, &[key::Right]),
    k(115, "END", OneLevel, &[key::End]),
    k(116, "DOWN", OneLevel, &[key::Down]),
    k(117, "PGDN", OneLevel, &[key::Next]),
    k(118, "INS", OneLevel, &[key::Insert]),
    k(119, "DELE", OneLevel, &[key::Delete]),
    k(121, "MUTE", OneLevel, &[key::XF86_AudioMute]),
    k(122, "VOL-", OneLevel, &[key::XF86_AudioLowerVolume]),
    k(123, "VOL+", OneLevel, &[key::XF86_AudioRaiseVolume]),
    k(127, "PAUS", OneLevel, &[key::Pause]),
    k(133, "LWIN", OneLevel, &[key::Super_L]),
    k(134, "RWIN", OneLevel, &[key::Super_R]),
    k(135, "MENU", OneLevel, &[key::Menu]),
];

/// Look up a key by XKB keycode.
#[must_use]
pub fn lookup(keycode: u32) -> Option<&'static KeyEntry> {
    US.binary_search_by_key(&keycode, |e| e.keycode).ok().map(|i| &US[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_so_binary_search_is_valid() {
        // `lookup` binary-searches. An unsorted table would silently miss keys
        // rather than fail loudly, so this is the guard on that assumption.
        for w in US.windows(2) {
            assert!(w[0].keycode < w[1].keycode, "unsorted at {}", w[0].keycode);
        }
    }

    #[test]
    fn every_key_has_a_usable_level_count() {
        for e in US {
            assert!(
                matches!(e.levels.len(), 1 | 2 | 4),
                "{} has {} levels",
                e.name,
                e.levels.len()
            );
            if e.kind == OneLevel {
                assert_eq!(e.levels.len(), 1, "{} is OneLevel but has levels", e.name);
            }
        }
    }

    #[test]
    fn key_names_are_unique() {
        // Duplicate names would emit a keymap that clients reject.
        let mut names: Vec<_> = US.iter().map(|e| e.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate key name in table");
    }

    #[test]
    fn a_is_lowercase_at_base_and_uppercase_at_shift() {
        let e = lookup(38).expect("AC01 present");
        assert_eq!(e.levels[0], key::a);
        assert_eq!(e.levels[1], key::A);
        assert_eq!(e.kind, Alphabetic);
    }

    #[test]
    fn digits_are_two_level_not_alphabetic() {
        // Caps Lock must not turn 1 into !. This is the bug the type system
        // in XKB exists to prevent, so it gets a test.
        let e = lookup(10).expect("AE01 present");
        assert_eq!(e.kind, TwoLevel);
    }

    #[test]
    fn lookup_misses_return_none() {
        assert!(lookup(0).is_none());
        assert!(lookup(200).is_none());
    }
}
