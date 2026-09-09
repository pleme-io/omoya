//! The keycode → keysym table.
//!
//! XKB keycode = evdev code + 8. Each entry carries the XKB key *name* as well,
//! because the same table emits the keymap text handed to Wayland clients — one
//! source, so the table we resolve against and the table clients compile can
//! never disagree.

use xkeysym::RawKeysym;
use xkeysym::key;

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
    /// Shift selects level 2; `AltGr` selects 3; both select 4. Caps ignored.
    ///
    /// The non-alphabetic four-level type — digits and punctuation on a layout
    /// that puts symbols on `AltGr`.
    FourLevel,
    /// Shift XOR Caps selects level 2; `AltGr` adds 2. Letters, four levels.
    ///
    /// Distinct from [`Self::FourLevel`] for exactly the reason
    /// [`Self::Alphabetic`] is distinct from [`Self::TwoLevel`]: Caps Lock must
    /// reach `ç` and must not reach `¹`.
    FourLevelAlphabetic,
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
            Self::FourLevel => "FOUR_LEVEL",
            Self::FourLevelAlphabetic => "FOUR_LEVEL_ALPHABETIC",
        }
    }

    /// How many keysym levels a key of this type MUST declare.
    ///
    /// ★ This is the seam that used to be a comment. A four-level type over a
    /// two-entry `levels` slice compiles fine, emits a keymap clients accept,
    /// and then silently produces nothing on `AltGr` — the exact shape of
    /// failure this crate exists to refuse. `levels_ok` is asserted for every
    /// key of every registered layout by `layout_tables_are_well_formed`.
    #[must_use]
    pub const fn levels_ok(self, n: usize) -> bool {
        match self {
            Self::OneLevel => n == 1,
            Self::TwoLevel | Self::Alphabetic | Self::Keypad => n == 2,
            Self::FourLevel | Self::FourLevelAlphabetic => n == 4,
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
    KeyEntry {
        keycode,
        name,
        kind,
        levels,
    }
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

/// Look up a key by XKB keycode in the `us` table.
///
/// Retained for callers that predate the registry. New code should go through
/// [`LayoutDef::lookup`] so the layout is a value rather than an assumption.
#[must_use]
pub fn lookup(keycode: u32) -> Option<&'static KeyEntry> {
    US.binary_search_by_key(&keycode, |e| e.keycode)
        .ok()
        .map(|i| &US[i])
}

// ── The registry ──────────────────────────────────────────────────────────
//
// ★ WHY A REGISTRY AND NOT A SECOND `if`.
//
// Until this existed, `layout` was not a parameter of this crate — it was a
// constant with a name. `xkbcommon-hairetsu::new_from_names` matched the
// literal `"us"` and returned `None` for everything else, so a node that
// declared `br` got `us` at the seat and `br` at the TTY, and nothing in
// between said so. (Measured on `ggg`, whose `services.xserver.xkb.layout` is
// `"br"`; the fallback is structural, so the seat came up and the operator's
// `ç` key typed `;`.)
//
// Adding `br` as a second special case would have left the class open. A
// registry makes a layout a ROW: `by_rmlvo` resolves against the table, the
// conformance matrix below runs every property against every row, and a row
// added without satisfying them fails the build rather than shipping a
// silently-wrong keyboard.

/// One layout: the RMLVO name callers ask for, the human name clients see, and
/// the table.
#[derive(Debug, Clone, Copy)]
pub struct LayoutDef {
    /// The RMLVO layout name, as it appears in `services.xserver.xkb.layout`.
    pub rmlvo: &'static str,
    /// `name[Group1]` in the emitted keymap — what a client displays.
    pub display: &'static str,
    /// The key table, ordered by keycode so lookup is a binary search.
    pub keys: &'static [KeyEntry],
}

impl LayoutDef {
    /// Look up a key by XKB keycode.
    #[must_use]
    pub fn lookup(&self, keycode: u32) -> Option<&'static KeyEntry> {
        self.keys
            .binary_search_by_key(&keycode, |e| e.keycode)
            .ok()
            .map(|i| &self.keys[i])
    }
}

/// Every layout this crate can compile.
///
/// ★ Ordered with `us` first because an empty layout name means "system
/// default" and resolves to `LAYOUTS[0]`.
pub static LAYOUTS: &[LayoutDef] = &[
    LayoutDef {
        rmlvo: "us",
        display: "English (US)",
        keys: US,
    },
    LayoutDef {
        rmlvo: "br",
        display: "Portuguese (Brazil)",
        keys: BR,
    },
];

/// Resolve an RMLVO layout name. An empty name is the system default.
#[must_use]
pub fn by_rmlvo(name: &str) -> Option<&'static LayoutDef> {
    if name.is_empty() {
        return LAYOUTS.first();
    }
    LAYOUTS.iter().find(|l| l.rmlvo == name)
}

// ── `br` (ABNT2) ──────────────────────────────────────────────────────────
//
// ★ TRANSCRIBED FROM xkeyboard-config 2.46, NOT RECALLED.
//
// Source: `share/X11/xkb/symbols/br`, the `abnt2` block, which is the `br`
// DEFAULT — there is no variant literally named "abnt2", which is why
// `nodes/ggg/default.nix` is right to declare bare `br`. That block is
// `include "latin"` plus 21 overrides plus `level3(ralt_switch)` and
// `kpdl(comma)`, so this table is:
//
//   US (the `pc` keys: function row, modifiers, keypad, navigation)
//     ⊕ latin(basic)  — the 28 keys latin gives four levels
//     ⊕ br(abnt2)     — the 21 keys Brazil overrides
//     ⊕ level3(ralt_switch) — RALT becomes ISO_Level3_Shift, ONE_LEVEL
//     ⊕ kpdl(comma)         — the numpad separator is a comma, not a period
//
// Keycodes were read from `share/X11/xkb/keycodes/evdev`, not derived. The one
// key US does not have at all is <AB11> = 97, the extra key ABNT2 puts left of
// the right Shift — it is where `/` and `?` live, which is why a Brazilian
// keyboard has `;` on AB10 and not on AC10.
//
// ★ RALT IS NOT Alt_R HERE. `level3(ralt_switch)` rebinds it, and
// `keysym_to_modifier` already maps `ISO_Level3_Shift` to MOD5, so the AltGr
// column below is reachable the moment this table declares it. That resolver
// arm has existed since the crate was written with no layout that could reach
// it — this table is its first consumer.

/// LATIN CAPITAL LETTER SHARP S, as a Unicode-form keysym.
///
/// ★ `xkeysym` has no name for this one. XKB encodes any Unicode codepoint
/// with no legacy keysym as `0x0100_0000 | codepoint`, and `keysym_name`
/// already falls back to the numeric form for unnamed syms, so emission is
/// correct without a name.
const U1E9E: RawKeysym = 0x0100_1E9E;
/// BULLET, as a Unicode-form keysym. See [`U1E9E`].
const U2022: RawKeysym = 0x0100_2022;

use KeyType::{FourLevel as F4, FourLevelAlphabetic as F4A};

/// The `br` layout — Brazilian ABNT2.
///
/// Ordered by keycode so lookup is a binary search.
pub static BR: &[KeyEntry] = &[
    k(9, "ESC", OneLevel, &[key::Escape]),
    // AE01 is NOT overridden by br(abnt2); these four come from latin(basic).
    k(
        10,
        "AE01",
        F4,
        &[key::_1, key::exclam, key::onesuperior, key::exclamdown],
    ),
    k(
        11,
        "AE02",
        F4,
        &[key::_2, key::at, key::twosuperior, key::onehalf],
    ),
    k(
        12,
        "AE03",
        F4,
        &[
            key::_3,
            key::numbersign,
            key::threesuperior,
            key::threequarters,
        ],
    ),
    k(
        13,
        "AE04",
        F4,
        &[key::_4, key::dollar, key::sterling, key::onequarter],
    ),
    k(
        14,
        "AE05",
        F4,
        &[key::_5, key::percent, key::cent, key::threeeighths],
    ),
    // ★ Shift+6 is a DEAD KEY on ABNT2, not `^`. This is the single most
    // surprising row for a reader used to `us`, and getting it wrong makes
    // every accented vowel unreachable.
    k(
        15,
        "AE06",
        F4,
        &[key::_6, key::dead_diaeresis, key::notsign, key::diaeresis],
    ),
    k(
        16,
        "AE07",
        F4,
        &[key::_7, key::ampersand, key::braceleft, key::seveneighths],
    ),
    k(
        17,
        "AE08",
        F4,
        &[key::_8, key::asterisk, key::bracketleft, key::trademark],
    ),
    k(
        18,
        "AE09",
        F4,
        &[key::_9, key::parenleft, key::bracketright, key::plusminus],
    ),
    k(
        19,
        "AE10",
        F4,
        &[key::_0, key::parenright, key::braceright, key::degree],
    ),
    k(
        20,
        "AE11",
        F4,
        &[
            key::minus,
            key::underscore,
            key::backslash,
            key::questiondown,
        ],
    ),
    k(
        21,
        "AE12",
        F4,
        &[key::equal, key::plus, key::section, key::dead_ogonek],
    ),
    k(22, "BKSP", OneLevel, &[key::BackSpace]),
    k(23, "TAB", OneLevel, &[key::Tab]),
    k(24, "AD01", F4A, &[key::q, key::Q, key::slash, key::slash]),
    k(
        25,
        "AD02",
        F4A,
        &[key::w, key::W, key::question, key::question],
    ),
    k(26, "AD03", F4A, &[key::e, key::E, key::degree, key::degree]),
    k(
        27,
        "AD04",
        F4A,
        &[key::r, key::R, key::registered, key::registered],
    ),
    k(28, "AD05", F4A, &[key::t, key::T, key::tslash, key::Tslash]),
    k(29, "AD06", F4A, &[key::y, key::Y, key::leftarrow, key::yen]),
    k(
        30,
        "AD07",
        F4A,
        &[key::u, key::U, key::downarrow, key::uparrow],
    ),
    k(
        31,
        "AD08",
        F4A,
        &[key::i, key::I, key::rightarrow, key::idotless],
    ),
    k(32, "AD09", F4A, &[key::o, key::O, key::oslash, key::Oslash]),
    k(33, "AD10", F4A, &[key::p, key::P, key::thorn, key::THORN]),
    // ★ The acute/grave dead-key pair. On `us` this keycode is `[`/`{`.
    k(
        34,
        "AD11",
        F4,
        &[key::dead_acute, key::dead_grave, key::acute, key::grave],
    ),
    k(
        35,
        "AD12",
        F4,
        &[
            key::bracketleft,
            key::braceleft,
            key::ordfeminine,
            key::dead_macron,
        ],
    ),
    k(36, "RTRN", OneLevel, &[key::Return]),
    k(37, "LCTL", OneLevel, &[key::Control_L]),
    k(38, "AC01", F4A, &[key::a, key::A, key::ae, key::AE]),
    k(39, "AC02", F4A, &[key::s, key::S, key::ssharp, U1E9E]),
    k(40, "AC03", F4A, &[key::d, key::D, key::eth, key::ETH]),
    k(
        41,
        "AC04",
        F4A,
        &[key::f, key::F, key::dstroke, key::ordfeminine],
    ),
    k(42, "AC05", F4A, &[key::g, key::G, key::eng, key::ENG]),
    k(
        43,
        "AC06",
        F4A,
        &[key::h, key::H, key::hstroke, key::Hstroke],
    ),
    k(
        44,
        "AC07",
        F4A,
        &[key::j, key::J, key::dead_hook, key::dead_horn],
    ),
    k(45, "AC08", F4A, &[key::k, key::K, key::kra, key::ampersand]),
    k(
        46,
        "AC09",
        F4A,
        &[key::l, key::L, key::lstroke, key::Lstroke],
    ),
    // ★ THE Ç KEY. This is the row `ggg` was missing: on `us` keycode 47 is
    // `;`/`:`, and the whole reason ABNT2 exists is that it is `ç`/`Ç` here.
    // Alphabetic, because Caps Lock must reach it.
    k(
        47,
        "AC10",
        F4A,
        &[
            key::ccedilla,
            key::Ccedilla,
            key::dead_acute,
            key::dead_doubleacute,
        ],
    ),
    // ★ The tilde/circumflex dead-key pair — ã, õ, â, ê, ô all come from here.
    k(
        48,
        "AC11",
        F4,
        &[
            key::dead_tilde,
            key::dead_circumflex,
            key::asciitilde,
            key::asciicircum,
        ],
    ),
    k(
        49,
        "TLDE",
        F4,
        &[key::apostrophe, key::quotedbl, key::notsign, key::notsign],
    ),
    k(50, "LFSH", OneLevel, &[key::Shift_L]),
    k(
        51,
        "BKSL",
        F4,
        &[
            key::bracketright,
            key::braceright,
            key::masculine,
            key::masculine,
        ],
    ),
    k(
        52,
        "AB01",
        F4A,
        &[key::z, key::Z, key::guillemotleft, key::less],
    ),
    k(
        53,
        "AB02",
        F4A,
        &[key::x, key::X, key::guillemotright, key::greater],
    ),
    k(
        54,
        "AB03",
        F4A,
        &[key::c, key::C, key::copyright, key::copyright],
    ),
    k(
        55,
        "AB04",
        F4A,
        &[
            key::v,
            key::V,
            key::doublelowquotemark,
            key::singlelowquotemark,
        ],
    ),
    k(
        56,
        "AB05",
        F4A,
        &[
            key::b,
            key::B,
            key::leftdoublequotemark,
            key::leftsinglequotemark,
        ],
    ),
    k(
        57,
        "AB06",
        F4A,
        &[
            key::n,
            key::N,
            key::rightdoublequotemark,
            key::rightsinglequotemark,
        ],
    ),
    k(58, "AB07", F4A, &[key::m, key::M, key::mu, key::mu]),
    k(
        59,
        "AB08",
        F4,
        &[key::comma, key::less, U2022, key::multiply],
    ),
    k(
        60,
        "AB09",
        F4,
        &[
            key::period,
            key::greater,
            key::periodcentered,
            key::division,
        ],
    ),
    // ★ `;` lives HERE on ABNT2, not on AC10 — AC10 is `ç`.
    k(
        61,
        "AB10",
        F4,
        &[
            key::semicolon,
            key::colon,
            key::dead_belowdot,
            key::dead_abovedot,
        ],
    ),
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
    // ★ `kpdl(comma)`: the ABNT2 numpad separator is a COMMA. Brazil writes
    // 1,5 — a numpad that types `.` there is wrong in every spreadsheet.
    k(91, "KPDL", Keypad, &[key::KP_Delete, key::KP_Separator]),
    k(
        94,
        "LSGT",
        F4,
        &[key::backslash, key::bar, key::dead_caron, key::dead_breve],
    ),
    k(95, "FK11", OneLevel, &[key::F11]),
    k(96, "FK12", OneLevel, &[key::F12]),
    // ★ THE EXTRA ABNT2 KEY. `us` has no keycode 97 at all.
    k(
        97,
        "AB11",
        F4,
        &[key::slash, key::question, key::degree, key::questiondown],
    ),
    k(104, "KPEN", OneLevel, &[key::KP_Enter]),
    k(105, "RCTL", OneLevel, &[key::Control_R]),
    k(106, "KPDV", OneLevel, &[key::KP_Divide]),
    k(107, "PRSC", OneLevel, &[key::Print]),
    // ★ NOT Alt_R — see the level3(ralt_switch) note above.
    k(108, "RALT", OneLevel, &[key::ISO_Level3_Shift]),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── The conformance matrix ────────────────────────────────────────────
    //
    // ★ EVERY PROPERTY RUNS AGAINST EVERY ROW OF `LAYOUTS`. A layout added
    // without satisfying them fails the build; there is no per-layout test to
    // forget to write. This is the closed-loop shape: the denominator is the
    // registry itself, so it cannot fall behind the thing it measures.
    //
    // The count assertion below is the anti-vacuity half. Without it, a
    // refactor that emptied `LAYOUTS` would turn every loop into a no-op and
    // the whole matrix green.

    #[test]
    fn the_matrix_covers_every_registered_layout() {
        assert!(
            LAYOUTS.len() >= 2,
            "the matrix below iterates LAYOUTS — an empty or single registry \
             makes every property in this module vacuous"
        );
    }

    #[test]
    fn tables_are_sorted_so_binary_search_is_valid() {
        // `lookup` binary-searches. An unsorted table would silently miss keys
        // rather than fail loudly, so this is the guard on that assumption.
        for l in LAYOUTS {
            for w in l.keys.windows(2) {
                assert!(
                    w[0].keycode < w[1].keycode,
                    "{}: unsorted at {}",
                    l.rmlvo,
                    w[0].keycode
                );
            }
        }
    }

    #[test]
    fn every_key_declares_the_level_count_its_type_requires() {
        // ★ The type↔level-count agreement, promoted from "1 | 2 | 4" to the
        // exact count the type implies. A FOUR_LEVEL key with two keysyms
        // emits a keymap clients accept and then produces nothing on AltGr.
        for l in LAYOUTS {
            for e in l.keys {
                assert!(
                    e.kind.levels_ok(e.levels.len()),
                    "{}: {} is {:?} but declares {} levels",
                    l.rmlvo,
                    e.name,
                    e.kind,
                    e.levels.len()
                );
            }
        }
    }

    #[test]
    fn key_names_are_unique_within_each_layout() {
        // Duplicate names would emit a keymap that clients reject.
        for l in LAYOUTS {
            let mut names: Vec<_> = l.keys.iter().map(|e| e.name).collect();
            names.sort_unstable();
            let before = names.len();
            names.dedup();
            assert_eq!(before, names.len(), "{}: duplicate key name", l.rmlvo);
        }
    }

    #[test]
    fn every_layout_can_be_typed_on() {
        // ★ A layout that cannot reach Return, space or a letter is a table
        // someone half-transcribed. Cheap, and it catches the shape where a
        // keycode was renumbered and the whole tail shifted.
        for l in LAYOUTS {
            for (code, what) in [(36, "Return"), (65, "space"), (38, "AC01"), (50, "LFSH")] {
                assert!(l.lookup(code).is_some(), "{}: no {what}", l.rmlvo);
            }
        }
    }

    #[test]
    fn a_four_level_layout_binds_a_level3_switch() {
        // ★ Declaring AltGr columns without a key that produces
        // ISO_Level3_Shift is the silent half of the four-level class: every
        // level-3 keysym is present in the table and none is reachable.
        for l in LAYOUTS {
            let has_four_level = l.keys.iter().any(|e| e.levels.len() == 4);
            if !has_four_level {
                continue;
            }
            let binds_switch = l
                .keys
                .iter()
                .any(|e| e.levels.contains(&key::ISO_Level3_Shift));
            assert!(
                binds_switch,
                "{}: has four-level keys but no ISO_Level3_Shift key — the \
                 AltGr columns are unreachable",
                l.rmlvo
            );
        }
    }

    #[test]
    fn rmlvo_names_are_unique_and_resolvable() {
        for l in LAYOUTS {
            let found = by_rmlvo(l.rmlvo).expect("registered layout resolves");
            assert_eq!(found.rmlvo, l.rmlvo);
        }
        let mut names: Vec<_> = LAYOUTS.iter().map(|l| l.rmlvo).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate rmlvo name in LAYOUTS");
    }

    #[test]
    fn the_empty_layout_name_is_the_system_default() {
        assert_eq!(by_rmlvo("").map(|l| l.rmlvo), Some("us"));
    }

    #[test]
    fn an_unregistered_layout_resolves_to_nothing() {
        // Refusal, not a silent substitution. Returning `us` for `de` would be
        // the worst failure this crate could have.
        assert!(by_rmlvo("de").is_none());
        assert!(by_rmlvo("US").is_none(), "resolution is case-sensitive");
    }

    #[test]
    fn br_puts_ccedilla_where_us_puts_semicolon() {
        // ★ The single row that is the whole point of ABNT2, and the one a
        // reader will want to see asserted rather than argued.
        let br = by_rmlvo("br").expect("br is registered");
        let us = by_rmlvo("us").expect("us is registered");
        assert_eq!(us.lookup(47).expect("us AC10").levels[0], key::semicolon);
        let cedilla = br.lookup(47).expect("br AC10");
        assert_eq!(cedilla.levels[0], key::ccedilla);
        assert_eq!(cedilla.levels[1], key::Ccedilla);
        assert_eq!(cedilla.kind, F4A, "Caps Lock must reach Ç");
        // ...and `;` moved to AB10, which on `us` is `/`.
        assert_eq!(br.lookup(61).expect("br AB10").levels[0], key::semicolon);
        assert_eq!(us.lookup(61).expect("us AB10").levels[0], key::slash);
    }

    #[test]
    fn br_has_the_extra_abnt2_key_and_us_does_not() {
        assert!(by_rmlvo("us").expect("us").lookup(97).is_none());
        let extra = by_rmlvo("br").expect("br").lookup(97).expect("AB11");
        assert_eq!(extra.name, "AB11");
        assert_eq!(extra.levels[0], key::slash);
    }

    #[test]
    fn br_rebinds_right_alt_to_the_level3_switch() {
        // level3(ralt_switch). If this regresses to Alt_R, every AltGr column
        // in the table becomes dead weight and nothing else fails.
        let br = by_rmlvo("br").expect("br");
        assert_eq!(
            br.lookup(108).expect("RALT").levels[0],
            key::ISO_Level3_Shift
        );
        assert_eq!(
            by_rmlvo("us")
                .expect("us")
                .lookup(108)
                .expect("RALT")
                .levels[0],
            key::Alt_R,
            "us keeps a plain right Alt"
        );
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
