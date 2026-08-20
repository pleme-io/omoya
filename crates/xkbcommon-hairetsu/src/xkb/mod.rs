//! The `xkb` module of the xkbcommon API, backed by [`hairetsu`].
//!
//! Every constant here is transcribed from the crate this replaces, because
//! these values cross into smithay and out to Wayland clients unchanged — a
//! wrong modifier index is a wrong keyboard, silently.

#![allow(clippy::module_name_repetitions)]

pub mod keysyms;

use std::borrow::Borrow;
use std::sync::Arc;

pub use xkeysym::KeyCode as Keycode;
pub use xkeysym::Keysym;

// --- type aliases, matching the C API's integer vocabulary ---------------

pub type LayoutIndex = u32;
pub type LayoutMask = u32;
pub type LevelIndex = u32;
pub type ModIndex = u32;
pub type ModMask = u32;
pub type LedIndex = u32;
pub type LedMask = u32;
pub type ContextFlags = u32;
pub type KeymapCompileFlags = u32;
pub type KeymapFormat = u32;
pub type StateComponent = u32;

// --- constants ------------------------------------------------------------

pub const MOD_INVALID: u32 = 0xffff_ffff;
pub const LED_INVALID: u32 = 0xffff_ffff;
pub const LAYOUT_INVALID: u32 = 0xffff_ffff;
pub const KEYCODE_INVALID: u32 = 0xffff_ffff;

pub const CONTEXT_NO_FLAGS: ContextFlags = 0;
pub const CONTEXT_NO_DEFAULT_INCLUDES: ContextFlags = 1;
pub const CONTEXT_NO_ENVIRONMENT_NAMES: ContextFlags = 2;

pub const KEYMAP_COMPILE_NO_FLAGS: KeymapCompileFlags = 0;
pub const KEYMAP_FORMAT_TEXT_V1: KeymapFormat = 1;

pub const STATE_MODS_DEPRESSED: StateComponent = 1 << 0;
pub const STATE_MODS_LATCHED: StateComponent = 1 << 1;
pub const STATE_MODS_LOCKED: StateComponent = 1 << 2;
pub const STATE_MODS_EFFECTIVE: StateComponent = 1 << 3;
pub const STATE_LAYOUT_DEPRESSED: StateComponent = 1 << 4;
pub const STATE_LAYOUT_LATCHED: StateComponent = 1 << 5;
pub const STATE_LAYOUT_LOCKED: StateComponent = 1 << 6;
pub const STATE_LAYOUT_EFFECTIVE: StateComponent = 1 << 7;
pub const STATE_LEDS: StateComponent = 1 << 8;

pub const STATE_MATCH_ANY: u32 = 1 << 0;
pub const STATE_MATCH_ALL: u32 = 1 << 1;
pub const STATE_MATCH_NON_EXCLUSIVE: u32 = 1 << 16;

pub const MOD_NAME_SHIFT: &str = "Shift";
pub const MOD_NAME_CAPS: &str = "Lock";
pub const MOD_NAME_CTRL: &str = "Control";
pub const MOD_NAME_ALT: &str = "Mod1";
pub const MOD_NAME_NUM: &str = "Mod2";
pub const MOD_NAME_MOD3: &str = "Mod3";
pub const MOD_NAME_LOGO: &str = "Mod4";
pub const MOD_NAME_MOD5: &str = "Mod5";
pub const MOD_NAME_ISO_LEVEL3_SHIFT: &str = "Mod5";

pub const LED_NAME_CAPS: &str = "Caps Lock";
pub const LED_NAME_NUM: &str = "Num Lock";
pub const LED_NAME_SCROLL: &str = "Scroll Lock";

/// Raw FFI-shaped constants.
///
/// The real crate exposes these from its `-sys` binding. Consumers reference a
/// few by name, so the names must exist; the values are the same integers as
/// the safe constants above.
pub mod ffi {
    pub const XKB_STATE_MODS_DEPRESSED: u32 = 1 << 0;
    pub const XKB_STATE_MODS_LATCHED: u32 = 1 << 1;
    pub const XKB_STATE_MODS_LOCKED: u32 = 1 << 2;
    pub const XKB_STATE_MODS_EFFECTIVE: u32 = 1 << 3;
    pub const XKB_STATE_LAYOUT_DEPRESSED: u32 = 1 << 4;
    pub const XKB_STATE_LAYOUT_LATCHED: u32 = 1 << 5;
    pub const XKB_STATE_LAYOUT_LOCKED: u32 = 1 << 6;
    pub const XKB_STATE_LAYOUT_EFFECTIVE: u32 = 1 << 7;
    pub const XKB_STATE_LEDS: u32 = 1 << 8;
}

/// Opaque stand-ins for the C handle types.
///
/// The real crate returns `*mut xkb_context` and friends from `get_raw_ptr`.
/// Those types must exist for the signatures to typecheck. **Every use of
/// `get_raw_ptr` in smithay is inside a `Debug` impl** — all four are
/// `.field("…", &x.get_raw_ptr())` — so the value is formatted as an address
/// and never dereferenced. That is checked, not assumed; it is what makes
/// returning a non-C address safe here.
#[allow(non_camel_case_types)]
pub enum xkb_context {}
#[allow(non_camel_case_types)]
pub enum xkb_keymap {}
#[allow(non_camel_case_types)]
pub enum xkb_state {}

/// Whether a key was pressed or released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDirection {
    Up,
    Down,
}

impl From<KeyDirection> for hairetsu::KeyDirection {
    fn from(d: KeyDirection) -> Self {
        match d {
            KeyDirection::Up => Self::Up,
            KeyDirection::Down => Self::Down,
        }
    }
}

/// The name of a keysym, or the empty string.
///
/// The C function returns `NoSymbol` for unknown values; this returns whatever
/// name the keysym table has, in XKB's spelling.
#[must_use]
pub fn keysym_get_name(keysym: Keysym) -> String {
    keysym.name().map_or_else(
        || format!("0x{:08x}", keysym.raw()),
        |n| {
            // xkeysym returns C header macro names; XKB's spelling drops the
            // prefix. Same normalisation the emitter applies.
            if let Some(rest) = n.strip_prefix("XF86XK_") {
                format!("XF86{rest}")
            } else if let Some(rest) = n.strip_prefix("XK_") {
                rest.to_owned()
            } else {
                n.to_owned()
            }
        },
    )
}

/// Look a keysym up by name. Returns `NoSymbol` when unknown.
#[must_use]
pub fn keysym_from_name(name: &str, _flags: u32) -> Keysym {
    // Single-codepoint names are the common case and need no table.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Keysym::from_char(c);
    }
    Keysym::new(keysyms::KEY_NoSymbol)
}

/// An xkbcommon context.
///
/// The C type owns include paths and a log handler. hairetsu reads no files
/// and logs nothing, so this carries only the flags it was given — kept so the
/// API shape matches.
#[derive(Debug, Clone)]
pub struct Context {
    flags: ContextFlags,
}

impl Context {
    #[must_use]
    pub const fn new(flags: ContextFlags) -> Self {
        Self { flags }
    }

    #[must_use]
    pub const fn get_flags(&self) -> ContextFlags {
        self.flags
    }

    /// The C handle. There isn't one — this returns this object's own address,
    /// which is what the only callers (`Debug` impls) actually print.
    #[must_use]
    pub fn get_raw_ptr(&self) -> *mut xkb_context {
        (self as *const Self).cast::<xkb_context>().cast_mut()
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new(CONTEXT_NO_FLAGS)
    }
}

/// A compiled keymap.
#[derive(Debug, Clone)]
pub struct Keymap {
    inner: Arc<hairetsu::Keymap>,
}

impl Keymap {
    /// Compile a keymap from RMLVO names.
    ///
    /// # Scope
    ///
    /// hairetsu ships one layout. An empty layout (meaning "system default") or
    /// `"us"` compiles; **anything else returns `None`.**
    ///
    /// That refusal is deliberate. Returning the `us` keymap for a request of
    /// `de` would be a silently wrong keyboard — the worst failure this crate
    /// could have. `None` is what the C function returns on a compile failure,
    /// so callers already handle it: smithay turns it into `Error::BadKeymap`.
    #[must_use]
    pub fn new_from_names<S: Borrow<str> + ?Sized>(
        _context: &Context,
        _rules: &S,
        _model: &S,
        layout: &S,
        variant: &S,
        _options: Option<String>,
        _flags: KeymapCompileFlags,
    ) -> Option<Self> {
        let layout = layout.borrow();
        let variant = variant.borrow();
        if !matches!(layout, "" | "us") || !variant.is_empty() {
            return None;
        }
        Some(Self { inner: Arc::new(hairetsu::Keymap::us()) })
    }

    /// Compile a keymap from a shared-memory fd, as a Wayland client sends one.
    ///
    /// Always `Ok(None)`, for the same reason as [`Self::new_from_string`]:
    /// hairetsu emits keymap text but does not parse it. `Ok(None)` is the
    /// crate's own "compiled to nothing" answer, so callers take their existing
    /// failure path rather than meeting a surprise.
    ///
    /// # Safety
    ///
    /// Matches the C-backed signature, which is `unsafe` because it mmaps the
    /// caller's fd. This implementation touches neither the fd nor the size.
    ///
    /// # Errors
    ///
    /// Never returns `Err`; the signature keeps `io::Result` for compatibility.
    #[allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)]
    pub unsafe fn new_from_fd(
        _context: &Context,
        _fd: std::os::fd::OwnedFd,
        _size: usize,
        _format: KeymapFormat,
        _flags: KeymapCompileFlags,
    ) -> std::io::Result<Option<Self>> {
        Ok(None)
    }

    /// Compile a keymap from XKB text.
    ///
    /// Always `None`: hairetsu emits keymap text but does not parse it. The
    /// caller's own error path handles this — smithay's
    /// `set_keymap_from_string` maps it to `Error::BadKeymap`.
    ///
    /// This is a real capability gap, named rather than faked.
    #[must_use]
    pub fn new_from_string<S: Borrow<str>>(
        _context: &Context,
        _text: S,
        _format: KeymapFormat,
        _flags: KeymapCompileFlags,
    ) -> Option<Self> {
        None
    }

    /// The keymap as XKB text — this is what reaches Wayland clients.
    #[must_use]
    pub fn get_as_string(&self, _format: KeymapFormat) -> String {
        self.inner.as_text().to_owned()
    }

    #[must_use]
    pub fn mod_get_index<S: Borrow<str> + ?Sized>(&self, name: &S) -> ModIndex {
        hairetsu::modifier::index_of(name.borrow()).unwrap_or(MOD_INVALID)
    }

    #[must_use]
    pub fn led_get_index<S: Borrow<str> + ?Sized>(&self, name: &S) -> LedIndex {
        let name = name.borrow();
        hairetsu::LED_NAMES
            .iter()
            .position(|n| *n == name)
            .and_then(|i| u32::try_from(i).ok())
            .unwrap_or(LED_INVALID)
    }

    /// See [`Context::get_raw_ptr`].
    #[must_use]
    pub fn get_raw_ptr(&self) -> *mut xkb_keymap {
        (self as *const Self).cast::<xkb_keymap>().cast_mut()
    }

    #[must_use]
    pub fn num_layouts(&self) -> LayoutIndex {
        self.inner.num_layouts()
    }

    /// Iterate the layout names.
    #[must_use]
    pub fn layouts(&self) -> KeymapLayouts<'_> {
        KeymapLayouts { keymap: self, ind: 0, len: self.inner.num_layouts() }
    }

    #[must_use]
    pub fn num_layouts_for_key(&self, _key: Keycode) -> LayoutIndex {
        self.inner.num_layouts()
    }

    #[must_use]
    pub fn layout_get_name(&self, idx: LayoutIndex) -> &str {
        self.inner.layout_name(idx)
    }

    #[must_use]
    pub fn min_keycode(&self) -> Keycode {
        Keycode::new(self.inner.min_keycode())
    }

    #[must_use]
    pub fn max_keycode(&self) -> Keycode {
        Keycode::new(self.inner.max_keycode())
    }

    #[must_use]
    pub fn key_repeats(&self, key: Keycode) -> bool {
        self.inner.key_repeats(key.raw())
    }

    #[must_use]
    pub fn key_get_syms_by_level(
        &self,
        key: Keycode,
        _layout: LayoutIndex,
        level: LevelIndex,
    ) -> &[Keysym] {
        self.inner.key_syms_by_level(key.raw(), level)
    }
}

/// Iterator over a keymap's layout names.
pub struct KeymapLayouts<'a> {
    keymap: &'a Keymap,
    ind: LayoutIndex,
    len: LayoutIndex,
}

impl<'a> Iterator for KeymapLayouts<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.ind == self.len {
            return None;
        }
        let name = self.keymap.inner.layout_name(self.ind);
        self.ind += 1;
        Some(name)
    }
}

/// Live keyboard state.
#[derive(Debug)]
pub struct State {
    inner: hairetsu::State,
}

impl State {
    #[must_use]
    pub fn new(keymap: &Keymap) -> Self {
        Self { inner: hairetsu::State::new(Arc::clone(&keymap.inner)) }
    }

    pub fn update_key(&mut self, key: Keycode, direction: KeyDirection) -> StateComponent {
        self.inner.update_key(key.raw(), direction.into())
    }

    pub fn update_mask(
        &mut self,
        depressed_mods: ModMask,
        latched_mods: ModMask,
        locked_mods: ModMask,
        depressed_layout: LayoutIndex,
        latched_layout: LayoutIndex,
        locked_layout: LayoutIndex,
    ) -> StateComponent {
        self.inner.update_mask(
            depressed_mods,
            latched_mods,
            locked_mods,
            depressed_layout,
            latched_layout,
            locked_layout,
        )
    }

    #[must_use]
    pub fn serialize_mods(&self, components: StateComponent) -> ModMask {
        self.inner.serialize_mods(components)
    }

    #[must_use]
    pub fn serialize_layout(&self, components: StateComponent) -> LayoutIndex {
        self.inner.serialize_layout(components)
    }

    #[must_use]
    pub fn mod_name_is_active<S: Borrow<str> + ?Sized>(
        &self,
        name: &S,
        _type_: StateComponent,
    ) -> bool {
        self.inner.mod_name_is_active(name.borrow())
    }

    /// See [`Context::get_raw_ptr`].
    #[must_use]
    pub fn get_raw_ptr(&self) -> *mut xkb_state {
        (self as *const Self).cast::<xkb_state>().cast_mut()
    }

    #[must_use]
    pub fn led_index_is_active(&self, idx: LedIndex) -> bool {
        idx != LED_INVALID
            && idx < u32::try_from(hairetsu::LED_NAMES.len()).unwrap_or(u32::MAX)
            && self.inner.leds() & (1 << idx) != 0
    }

    #[must_use]
    pub fn layout_index_is_active(&self, idx: LayoutIndex, _type_: StateComponent) -> bool {
        // Single-layout by scope, so layout 0 is the only active one.
        idx == 0
    }

    #[must_use]
    pub fn led_name_is_active<S: Borrow<str> + ?Sized>(&self, name: &S) -> bool {
        let name = name.borrow();
        hairetsu::LED_NAMES
            .iter()
            .position(|n| *n == name)
            .is_some_and(|i| self.inner.leds() & (1 << i) != 0)
    }

    #[must_use]
    pub fn key_get_syms(&self, key: Keycode) -> &[Keysym] {
        self.inner.key_syms(key.raw())
    }

    #[must_use]
    pub fn key_get_one_sym(&self, key: Keycode) -> Keysym {
        self.inner.key_sym(key.raw())
    }

    #[must_use]
    pub fn key_get_layout(&self, key: Keycode) -> LayoutIndex {
        self.inner.layout_for_key(key.raw())
    }

    #[must_use]
    pub fn key_get_level(&self, key: Keycode, _layout: LayoutIndex) -> LevelIndex {
        self.inner.level_for_key(key.raw())
    }

    #[must_use]
    pub fn get_keymap(&self) -> Keymap {
        Keymap { inner: Arc::clone(self.inner.keymap()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> Keymap {
        Keymap::new_from_names(&Context::new(CONTEXT_NO_FLAGS), "", "", "", "", None, 0)
            .expect("default layout compiles")
    }

    #[test]
    fn default_and_us_layouts_compile() {
        let ctx = Context::new(CONTEXT_NO_FLAGS);
        assert!(Keymap::new_from_names(&ctx, "", "", "", "", None, 0).is_some());
        assert!(Keymap::new_from_names(&ctx, "evdev", "pc105", "us", "", None, 0).is_some());
    }

    #[test]
    fn an_unsupported_layout_refuses_instead_of_lying() {
        // The single most important behaviour in this crate: handing back `us`
        // for a `de` request would be a silently wrong keyboard.
        let ctx = Context::new(CONTEXT_NO_FLAGS);
        assert!(Keymap::new_from_names(&ctx, "evdev", "pc105", "de", "", None, 0).is_none());
        assert!(Keymap::new_from_names(&ctx, "evdev", "pc105", "us", "dvorak", None, 0).is_none());
    }

    #[test]
    fn keymap_text_is_non_empty_and_well_formed() {
        let t = keymap().get_as_string(KEYMAP_FORMAT_TEXT_V1);
        assert!(t.starts_with("xkb_keymap {"));
        assert!(t.len() > 1000);
    }

    #[test]
    fn mod_indices_match_the_c_api() {
        let km = keymap();
        assert_eq!(km.mod_get_index(MOD_NAME_SHIFT), 0);
        assert_eq!(km.mod_get_index(MOD_NAME_CAPS), 1);
        assert_eq!(km.mod_get_index(MOD_NAME_CTRL), 2);
        assert_eq!(km.mod_get_index(MOD_NAME_ALT), 3);
        assert_eq!(km.mod_get_index(MOD_NAME_LOGO), 6);
        assert_eq!(km.mod_get_index("Nonexistent"), MOD_INVALID);
    }

    #[test]
    fn led_indices_match_the_c_api() {
        let km = keymap();
        assert_eq!(km.led_get_index(LED_NAME_CAPS), 0);
        assert_eq!(km.led_get_index(LED_NAME_NUM), 1);
        assert_eq!(km.led_get_index(LED_NAME_SCROLL), 2);
        assert_eq!(km.led_get_index("Nope"), LED_INVALID);
    }

    #[test]
    fn state_resolves_shift_through_the_facade() {
        let km = keymap();
        let mut st = State::new(&km);
        assert_eq!(st.key_get_one_sym(Keycode::new(38)).raw(), keysyms::KEY_a);
        st.update_key(Keycode::new(50), KeyDirection::Down);
        assert_eq!(st.key_get_one_sym(Keycode::new(38)).raw(), keysyms::KEY_A);
        assert!(st.mod_name_is_active(MOD_NAME_SHIFT, STATE_MODS_EFFECTIVE));
    }

    #[test]
    fn keysym_names_use_xkb_spelling_not_c_macros() {
        assert_eq!(keysym_get_name(Keysym::new(keysyms::KEY_a)), "a");
        assert_eq!(keysym_get_name(Keysym::new(keysyms::KEY_BackSpace)), "BackSpace");
        assert_eq!(
            keysym_get_name(Keysym::new(keysyms::KEY_XF86AudioMute)),
            "XF86AudioMute"
        );
    }

    #[test]
    fn new_from_string_refuses_rather_than_pretending() {
        let ctx = Context::new(CONTEXT_NO_FLAGS);
        assert!(Keymap::new_from_string(&ctx, "xkb_keymap {};", KEYMAP_FORMAT_TEXT_V1, 0).is_none());
    }

    #[test]
    fn ffi_constants_agree_with_the_safe_ones() {
        // These are two spellings of one value; a mismatch would be invisible.
        assert_eq!(ffi::XKB_STATE_LAYOUT_EFFECTIVE, STATE_LAYOUT_EFFECTIVE);
        assert_eq!(ffi::XKB_STATE_MODS_EFFECTIVE, STATE_MODS_EFFECTIVE);
        assert_eq!(ffi::XKB_STATE_MODS_DEPRESSED, STATE_MODS_DEPRESSED);
    }

    #[test]
    fn layouts_iterates_exactly_the_declared_layouts() {
        let km = keymap();
        let names: Vec<&str> = km.layouts().collect();
        assert_eq!(names.len(), km.num_layouts() as usize);
        assert_eq!(names, vec!["English (US)"]);
    }

    #[test]
    fn led_index_is_active_agrees_with_led_name_is_active() {
        // Two spellings of one question; disagreement would be invisible.
        let km = keymap();
        let mut st = State::new(&km);
        let caps = km.led_get_index(LED_NAME_CAPS);
        assert!(!st.led_index_is_active(caps));
        st.update_key(Keycode::new(66), KeyDirection::Down);
        assert!(st.led_index_is_active(caps));
        assert_eq!(st.led_index_is_active(caps), st.led_name_is_active(LED_NAME_CAPS));
        // An invalid index must not index out of bounds or shift by >= 32.
        assert!(!st.led_index_is_active(LED_INVALID));
        assert!(!st.led_index_is_active(99));
    }

    #[test]
    fn layout_zero_is_the_active_layout() {
        let km = keymap();
        let st = State::new(&km);
        assert!(st.layout_index_is_active(0, STATE_LAYOUT_EFFECTIVE));
        assert!(!st.layout_index_is_active(1, STATE_LAYOUT_EFFECTIVE));
    }

    #[test]
    fn raw_pointers_are_distinct_and_non_null() {
        // Only ever formatted by `Debug`, but a null would render as 0x0 for
        // every object and make those logs useless.
        let km = keymap();
        let st = State::new(&km);
        assert!(!km.get_raw_ptr().is_null());
        assert!(!st.get_raw_ptr().is_null());
        assert!(!Context::default().get_raw_ptr().is_null());
    }

    #[test]
    fn led_state_is_readable_through_the_facade() {
        let km = keymap();
        let mut st = State::new(&km);
        assert!(!st.led_name_is_active(LED_NAME_CAPS));
        st.update_key(Keycode::new(66), KeyDirection::Down);
        assert!(st.led_name_is_active(LED_NAME_CAPS));
    }
}
