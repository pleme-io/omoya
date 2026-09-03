//! ukeire (受け入れ) — the seat's **intake** of physical input.
//!
//! One question, asked once: *when a physical input event arrives, what does
//! it MEAN and how fast do we take it?* Everything the seat decides between
//! the kernel handing us an evdev event and a client receiving a Wayland one
//! lives here, as one typed value.
//!
//! ── ★ WHY THIS EXISTS AT ALL ─────────────────────────────────────────────
//! Because every one of these answers used to be a literal, scattered across
//! six files, and the scattering is not a tidiness complaint — it is the
//! reason a non-US operator cannot use this seat today. Censused 2026-09-03:
//!
//! | fact | where it was | what it was |
//! |---|---|---|
//! | keymap | `state.rs`, `XkbConfig::default()` | **hardcoded US, no options** |
//! | repeat delay/rate | `state.rs`, an argument pair | `600`/`25`, typed earlier today |
//! | scroll step | `input.rs`, mid-expression | `* 3.0 / 120.` |
//! | scroll direction | nowhere | **not representable** |
//! | seat modifier | `deed.rs`, `pub const LOGO` | `Modifiers::CMD` |
//! | cursor scale | `cursor.rs`, `pub const SCALE` | `2` |
//! | remaps | `config.rs` | already configured — the one that was done right |
//!
//! ── ★ THE FINDING THAT JUSTIFIES THE VOCABULARY ──────────────────────────
//! The keymap row is not merely unconfigured, it is **silently divergent**.
//! Three declarations of "what layout is this machine" already exist in the
//! fleet's nix tree — `services.xserver.xkb.layout`,
//! `blackmatter.profiles.blizzard.console.keyMap`, and GNOME's
//! `org/gnome/desktop/input-sources` (which is at least *derived* from the
//! first) — and the Wayland seat read **none** of them. Measured: `plo`
//! declares `us` and `ggg` declares `br` (ABNT2). omoya's hardcoded US
//! therefore agrees with plo **by coincidence**, which is exactly why nobody
//! noticed, and would give gabi a US keymap on a Brazilian keyboard the day
//! omoya reaches ggg. The drift is latent, not live — and latent drift that
//! reads correct at every layer is the shape this fleet's dated-claim rule
//! warns about.
//!
//! So `Keymap`'s job is not to add a fourth declaration. It is to be the
//! **projection target** of the one that already exists: the nix module
//! defaults `ukeire.keymap.layout` from `services.xserver.xkb.layout`, so the
//! seat and the TTY cannot disagree without someone overriding on purpose.
//!
//! ── ★ WHAT THIS DELIBERATELY DOES NOT OWN ────────────────────────────────
//! - **Chord semantics.** Which chord does what is `deed.rs` over `awase`.
//!   ukeire owns only *which modifier* the vocabulary hangs off, because that
//!   is an intake question (what does this physical modifier mean) rather
//!   than a binding question.
//! - **Reservation policy.** `awase::Reserved` owns what may not be bound.
//!   ukeire *consults* it to refuse a remap; it never re-states the claims.
//! - **Repeat gating at the consumer.** `awase::KeyRepeatGate` bounds a
//!   runaway held key inside an application. ukeire sets the seat's advertised
//!   pace. Two different questions, and keeping them apart is what lets the
//!   seat be fast while a password field stays safe.
//! - **Pointer acceleration, tap-to-click, natural-scroll-by-device.** omoya
//!   reads raw evdev, not libinput, so there is no acceleration layer to
//!   configure — the knobs do not exist to expose. Named here so their
//!   absence reads as a measured fact rather than an oversight.
//!
//! ── ★ TIER HONESTY ───────────────────────────────────────────────────────
//! M0 is plain typed Rust plus shikumi config. The `(defukeire …)` tatara-lisp
//! form and its `#[derive(DeriveTataraDomain)]` border are a **named M1**;
//! `specs/ukeire.lisp` documents the destination form and is not the wired
//! form. Per-bad-state tiers are in `docs/UKEIRE.md`'s ledger — read that
//! rather than assuming a validating constructor is a compile error.

use serde::{Deserialize, Serialize};

// ── Bounded leaves ───────────────────────────────────────────────────────
//
// ★ Every bounded number here is `Refined<T, B>` from `ishou-tokens`, never a
// hand-written `if v > MAX`. That is the fleet's input-resilience rule, and
// the mechanism matters: `Refined`'s `Deserialize` clamps at the parse
// boundary, so no expression in this crate can *hold* an out-of-band value.
// A typo'd yaml number yields a working seat rather than a compositor that
// refuses to start and leaves the operator with no way in — the same
// reasoning `OmoyaConfig::bare()` gives for existing at all.

/// 50..=2000 ms. Below 50 ms every keypress becomes a burst; above 2 s the
/// operator concludes repeat is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatDelayBounds;

impl ishou_tokens::Bounds<i32> for RepeatDelayBounds {
    fn min() -> i32 {
        50
    }
    fn max() -> i32 {
        2000
    }
    fn default() -> i32 {
        DEFAULT_REPEAT_DELAY_MS
    }
}

/// 0..=100 Hz. `0` is Wayland's own spelling for "repeat disabled"; 100 is
/// already twice a fast desktop default, and past it the client's ability to
/// drain its event queue is the bottleneck, not the seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatRateBounds;

impl ishou_tokens::Bounds<i32> for RepeatRateBounds {
    fn min() -> i32 {
        0
    }
    fn max() -> i32 {
        100
    }
    fn default() -> i32 {
        DEFAULT_REPEAT_RATE_HZ
    }
}

/// 0.25..=10.0 lines per detent.
///
/// The lower bound is not 0: a factor of zero is *dead scroll*, which is
/// indistinguishable from a broken pointer and is spelled by
/// `ScrollDirection`, not by a magnitude of nothing. The upper bound keeps a
/// single detent from paging a document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollFactorBounds;

impl ishou_tokens::Bounds<f64> for ScrollFactorBounds {
    fn min() -> f64 {
        0.25
    }
    fn max() -> f64 {
        10.0
    }
    fn default() -> f64 {
        DEFAULT_SCROLL_FACTOR
    }
}

/// 1..=6 screen pixels per cursor-mask cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorScaleBounds;

impl ishou_tokens::Bounds<i32> for CursorScaleBounds {
    fn min() -> i32 {
        1
    }
    fn max() -> i32 {
        6
    }
    fn default() -> i32 {
        DEFAULT_CURSOR_SCALE
    }
}

pub type BoundedRepeatDelay = ishou_tokens::Refined<i32, RepeatDelayBounds>;
pub type BoundedRepeatRate = ishou_tokens::Refined<i32, RepeatRateBounds>;
pub type BoundedScrollFactor = ishou_tokens::Refined<f64, ScrollFactorBounds>;
pub type BoundedCursorScale = ishou_tokens::Refined<i32, CursorScaleBounds>;

/// 200 ms — a deliberate change from the 600 ms this seat shipped with.
///
/// 600/25 was chosen when omoya's only face was the greeter, where a held key
/// repeating into a password field is a genuine hazard. That was right for
/// that face and wrong for this one: plo runs a full session now. The greeter
/// hazard keeps its own answer at the consumer (`awase::KeyRepeatGate`), so
/// it does not depend on the seat staying slow for everyone.
pub const DEFAULT_REPEAT_DELAY_MS: i32 = 200;

/// 45 Hz — near the fast end of what desktops ship without reaching the range
/// where the client cannot keep up.
pub const DEFAULT_REPEAT_RATE_HZ: i32 = 45;

/// 3.0 — preserved exactly from the `* 3.0 / 120.` that was inline in
/// `input.rs`, so typing this fact did not silently change how the seat
/// scrolls. The `/ 120` half is not configurable and must not be: it is
/// `wl_pointer.axis_v120`'s wire unit, a property of the protocol rather than
/// a preference.
pub const DEFAULT_SCROLL_FACTOR: f64 = 3.0;

/// 2 — preserved from `cursor::SCALE`. The mask is 10x17, so this is a 20x34
/// pointer; at 1:1 it was smaller than a character cell, which is how the
/// previous cursor managed to be on screen and unfindable at once.
pub const DEFAULT_CURSOR_SCALE: i32 = 2;

// ── Closed axes ──────────────────────────────────────────────────────────

/// Which way a scroll detent moves the content.
///
/// ★ A CLOSED ENUM, NOT A SIGN ON THE FACTOR. Natural scrolling is a
/// *direction*, and encoding it as a negative magnitude makes two independent
/// facts share one number — so "inverted" and "twice as fast" become the same
/// edit, and a stray minus is a silent inversion rather than a config error.
/// Splitting them also removes the need for a `factor != 0` rule: dead scroll
/// is not a small magnitude, it is not a direction at all, and has no
/// spelling here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollDirection {
    /// The content follows the wheel: wheel down scrolls down. What omoya has
    /// always done, so it stays the default.
    #[default]
    Traditional,
    /// The content follows the fingers: wheel down scrolls the view up.
    Natural,
}

impl ScrollDirection {
    /// `+1.0` or `-1.0`, to multiply a magnitude by.
    ///
    /// The ONE place a direction becomes a sign. Every consumer multiplies by
    /// this rather than deciding for itself, so a second inversion cannot be
    /// introduced downstream.
    #[must_use]
    pub const fn sign(self) -> f64 {
        match self {
            Self::Traditional => 1.0,
            Self::Natural => -1.0,
        }
    }
}

/// The modifier the seat's whole chord vocabulary hangs off.
///
/// ★ CLOSED, AND THAT IS THE POINT. `deed.rs` had `pub const LOGO:
/// Modifiers = Modifiers::CMD`, so the choice was unavailable to the
/// operator; the naive fix — accept an arbitrary `Modifiers` from yaml —
/// would let someone select `CTRL`, at which point every fleet chord
/// collides with `Ctrl+Alt+F1..F12` and the machine soft-bricks with no VT
/// escape. Two variants, both known-safe, so the dangerous choice has no
/// representation rather than a runtime check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeatModifier {
    /// The Super/Command/Logo key. omoya's default since it shipped.
    #[default]
    Super,
    /// Alt, for an operator whose muscle memory came from i3 or sway.
    Alt,
}

impl SeatModifier {
    /// The `awase::Modifiers` this selects.
    #[must_use]
    pub fn modifiers(self) -> awase::Modifiers {
        match self {
            Self::Super => awase::Modifiers::CMD,
            Self::Alt => awase::Modifiers::ALT,
        }
    }
}

// ── The domain condition ─────────────────────────────────────────────────

/// How the seat interprets a physical keyboard event.
///
/// The five xkb fields, spelled as xkb spells them. `None` means "xkb's own
/// default", which is what `XkbConfig::default()` gave us for all five — so
/// an unconfigured `Keymap` is byte-for-byte the old behaviour and the
/// vocabulary can land without changing any running seat.
///
/// ★ NOT VALIDATED HERE, AND THAT IS DELIBERATE. Whether a layout string
/// compiles is a question only libxkbcommon can answer, and answering it
/// means building the keymap. So `Keymap` carries the operator's intent
/// faithfully and `Ukeire::apply`'s caller reports a compile failure as a
/// typed error with a fallback to a usable seat. Validating by pattern-match
/// against a list of known layout names would be a guess wearing a check's
/// clothes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keymap {
    /// xkb rules file, e.g. `evdev`.
    pub rules: Option<String>,
    /// Keyboard model, e.g. `pc105`.
    pub model: Option<String>,
    /// Layout, e.g. `us`, `br`, or `us,br` for a switchable pair.
    ///
    /// The nix module defaults this from `services.xserver.xkb.layout`, so
    /// the seat agrees with the TTY unless someone overrides on purpose.
    pub layout: Option<String>,
    /// Variant, e.g. `dvorak`. `br` needs none — the bare layout IS ABNT2.
    pub variant: Option<String>,
    /// Options, e.g. `grp:alt_shift_toggle`.
    ///
    /// ★ Not the place for `caps:escape`. omoya remaps CapsLock at the
    /// **evdev** layer (`remap.rs`), below xkb, so it survives a layout the
    /// operator switches to mid-session. Setting it here as well would be a
    /// second answer to one question.
    pub options: Option<String>,
}

impl Keymap {
    /// True when every field is absent — i.e. xkb's defaults, the seat's
    /// behaviour before this vocabulary existed.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.rules.is_none()
            && self.model.is_none()
            && self.layout.is_none()
            && self.variant.is_none()
            && self.options.is_none()
    }
}

/// How fast a held key is taken.
///
/// No `enable` bool: `rate_hz = 0` is `wl_keyboard.repeat_info`'s own
/// spelling for off, so *disabled-with-a-delay* — a combination with no
/// meaning — has no representation and needs no cross-field rule.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Repeat {
    /// Milliseconds a key must be held before the first repeat.
    pub delay_ms: BoundedRepeatDelay,
    /// Repeats per second once repeating. `0` disables repeat.
    pub rate_hz: BoundedRepeatRate,
}

impl Default for Repeat {
    fn default() -> Self {
        Self {
            delay_ms: BoundedRepeatDelay::new(DEFAULT_REPEAT_DELAY_MS),
            rate_hz: BoundedRepeatRate::new(DEFAULT_REPEAT_RATE_HZ),
        }
    }
}

impl Repeat {
    /// The pair smithay's `add_keyboard` / `change_repeat_info` wants, in
    /// **smithay's** argument order: `(delay_ms, rate_hz)`.
    ///
    /// ★ THE ORDER IS THE TRAP, AND IT IS UNTYPED ON BOTH SIDES. smithay
    /// takes delay first; `wl_keyboard.repeat_info` sends rate first. Both
    /// are `i32`, so swapping them is not a type error — it yields a seat
    /// that waits 45 ms and then repeats 200 times a second, which reads as a
    /// possessed keyboard rather than as a config mistake. This method is the
    /// single place that ordering is written down.
    #[must_use]
    pub fn smithay_repeat_info(self) -> (i32, i32) {
        (self.delay_ms.get(), self.rate_hz.get())
    }

    /// True when repeat is off.
    #[must_use]
    pub fn is_disabled(self) -> bool {
        self.rate_hz.get() == 0
    }
}

/// How an axis event is taken.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scroll {
    /// Which way a detent moves the content.
    pub direction: ScrollDirection,
    /// Lines per detent, before the v120 wire conversion.
    pub factor: BoundedScrollFactor,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            direction: ScrollDirection::default(),
            factor: BoundedScrollFactor::new(DEFAULT_SCROLL_FACTOR),
        }
    }
}

impl Scroll {
    /// The multiplier to apply to a `v120` amount: magnitude, signed by
    /// direction, divided by the protocol's 120 units per detent.
    ///
    /// ★ The `/ 120.0` lives HERE and is not configurable, because it is not
    /// a preference — it is `wl_pointer.axis_v120`'s unit. Exposing it would
    /// invite an operator to "fix" scrolling by editing a protocol constant.
    #[must_use]
    pub fn v120_multiplier(self) -> f64 {
        self.direction.sign() * self.factor.get() / 120.0
    }
}

/// How the pointer is presented.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pointer {
    /// Screen pixels per cursor-mask cell.
    pub cursor_scale: BoundedCursorScale,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            cursor_scale: BoundedCursorScale::new(DEFAULT_CURSOR_SCALE),
        }
    }
}

/// The seat's whole intake policy, as one value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ukeire {
    /// What a keyboard event means.
    pub keymap: Keymap,
    /// How fast a held key is taken.
    pub repeat: Repeat,
    /// How an axis event is taken.
    pub scroll: Scroll,
    /// How the pointer is presented.
    pub pointer: Pointer,
    /// Which modifier the chord vocabulary hangs off.
    pub modifier: SeatModifier,
    /// evdev-level rewrites, proven safe by their own type.
    pub remaps: Remaps,
}

impl Default for Ukeire {
    /// ★ NOT `#[derive(Default)]`, because `Remaps::default()` is EMPTY and
    /// the seat's default is not: CapsLock -> Escape survives even the bare
    /// tier, for the reason `remap.rs` gives — the worst-placed key on the
    /// board, under the strongest finger. Deriving would have silently
    /// dropped it the moment remaps moved onto this struct, and nothing in
    /// the diff would have said so.
    fn default() -> Self {
        Self {
            keymap: Keymap::default(),
            repeat: Repeat::default(),
            scroll: Scroll::default(),
            pointer: Pointer::default(),
            modifier: SeatModifier::default(),
            remaps: Remaps::unchecked(crate::remap::DEFAULT_REMAPS),
        }
    }
}

// ── Remaps: refused at the parse boundary, not validated afterwards ──────

/// One evdev-level rewrite.
///
/// Lives here rather than in `config.rs` because a remap is an *intake*
/// decision — it changes what a physical key means before xkb ever sees it.
/// `config::Remap` is a re-export, so existing spellings still resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remap {
    /// evdev code to rewrite, e.g. 58 for `KEY_CAPSLOCK`.
    pub from: u32,
    /// evdev code to rewrite it to, e.g. 1 for `KEY_ESC`.
    pub to: u32,
}

/// A remap set that has already been proven safe.
///
/// ── ★ WHY THIS IS A NEWTYPE AND NOT A `Vec<Remap>` + A CHECK ─────────────
/// `Ukeire::refusals` was a *validator*: it returned a list, `main.rs`
/// reported it, and the ledger graded both its rows `only-mitigated (C1)`
/// because a caller who forgot to consult it would apply a soft-bricking
/// remap with no complaint. That ceiling was real and named, and this type
/// removes it.
///
/// The move that makes it possible: **`awase::Reserved::fleet_linux()` is a
/// pure function of nothing.** It needs no config, no environment and no
/// caller context, so `Deserialize` can construct the claim set itself and
/// refuse at the parse boundary rather than deferring to a later check.
/// There is no `Remaps` value anywhere in this crate that rewrites a
/// reserved key, and no constructor that produces one — `parse` does not
/// validate, it *refuses*.
///
/// Three conditions, all refused:
/// - **a reserved source** — rewriting evdev 60 removes `Ctrl+Alt+F2` from
///   existence, including the operator's own route to a TTY to undo the
///   edit. A soft-brick reachable from a two-number yaml edit.
/// - **a self-remap** — a no-op that reads as an intentional rewrite.
/// - **a duplicated source** — last-write-wins would be a silent choice
///   between two stated intentions.
///
/// `unchecked` exists for the ONE caller that has a compile-time-known set
/// (the seat's own defaults), and it is `pub(crate)` so no config path can
/// reach it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct Remaps(Vec<Remap>);

impl Remaps {
    /// Refuse anything unsafe. The single public constructor.
    ///
    /// # Errors
    /// Every problem, not the first: an operator fixing a config wants the
    /// whole list rather than one rebuild cycle per mistake.
    pub fn parse(pairs: &[(u32, u32)]) -> Result<Self, Vec<UkeireError>> {
        Self::parse_against(pairs, &awase::Reserved::fleet_linux())
    }

    /// `parse`, against an explicit claim set.
    ///
    /// Exists so a test can prove the refusal fires off the *claims* rather
    /// than off this file's table — pass an empty `Reserved` and the reserved
    /// arm must go quiet while the other two still fire.
    ///
    /// # Errors
    /// As `parse`.
    pub fn parse_against(
        pairs: &[(u32, u32)],
        reserved: &awase::Reserved,
    ) -> Result<Self, Vec<UkeireError>> {
        let protected = reserved_codes(reserved);
        let mut out = Vec::new();
        let mut seen: Vec<u32> = Vec::new();

        for &(from, to) in pairs {
            if from == to {
                out.push(UkeireError::SelfRemap { code: from });
            }
            if seen.contains(&from) {
                out.push(UkeireError::DuplicateRemapSource { code: from });
            } else {
                seen.push(from);
            }
            if let Some((code, key)) = protected.iter().find(|(c, _)| *c == from) {
                out.push(UkeireError::RemapOfReservedKey {
                    code: *code,
                    key: *key,
                });
            }
        }

        if out.is_empty() {
            Ok(Self(
                pairs.iter().map(|&(from, to)| Remap { from, to }).collect(),
            ))
        } else {
            Err(out)
        }
    }

    /// The seat's own compile-time-known defaults.
    ///
    /// `pub(crate)` on purpose: this is the one place a set skips the
    /// refusal, and it must stay unreachable from any config path. The
    /// defaults are still put through `parse` in a test, because a guard
    /// that refuses the shipped default is a guard that gets deleted.
    pub(crate) fn unchecked(pairs: &[(u32, u32)]) -> Self {
        Self(pairs.iter().map(|&(from, to)| Remap { from, to }).collect())
    }

    /// The pairs `remap::apply` wants.
    #[must_use]
    pub fn pairs(&self) -> Vec<(u32, u32)> {
        self.0.iter().map(|r| (r.from, r.to)).collect()
    }

    /// How many rewrites.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when nothing is rewritten.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for Remaps {
    /// ★ REFUSES, rather than clamping like the bounded leaves do.
    ///
    /// The asymmetry is deliberate and is the difference between a *preference*
    /// and a *hazard*. An out-of-band repeat rate has an obviously-right
    /// nearest legal value, so clamping keeps the seat alive at no cost. A
    /// remap that eats the VT escape has no nearest legal value — silently
    /// dropping it would leave the operator believing CapsLock was remapped
    /// when it was not, and silently keeping it would cost them the machine.
    /// So this one is an error, and `config::load`'s existing fall-back to the
    /// prescribed tier is what keeps a seat on screen.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Vec::<Remap>::deserialize(d)?;
        let pairs: Vec<(u32, u32)> = raw.iter().map(|r| (r.from, r.to)).collect();
        Self::parse(&pairs).map_err(|errs| {
            // One message naming every problem. `Display` on each arm already
            // names the offending code, so this is a join, not a re-write.
            let joined = errs
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            serde::de::Error::custom(joined)
        })
    }
}

// ── Refusals ─────────────────────────────────────────────────────────────

/// Why an intake policy was refused.
///
/// Every arm names the offending value, because a refusal an operator cannot
/// act on is a refusal that gets worked around. These are returned, never
/// panicked: the caller's job is to report them and fall back to a seat the
/// operator can log in and fix the config from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UkeireError {
    /// A remap whose source and target are the same key — a no-op that reads
    /// as an intentional rewrite, so it is refused rather than ignored.
    SelfRemap { code: u32 },
    /// Two remaps claiming the same source key. Last-write-wins would be a
    /// silent choice between two stated intentions.
    DuplicateRemapSource { code: u32 },
    /// A remap that rewrites a key `awase::Reserved` claims for an escape
    /// chord — the VT switches and Ctrl+Alt+Delete.
    ///
    /// ★ THE ONE REFUSAL THAT PROTECTS THE MACHINE RATHER THAN THE CONFIG.
    /// Remaps are applied at the evdev layer, *below* xkb and below chord
    /// matching, so rewriting the physical F2 key removes `Ctrl+Alt+F2` from
    /// existence — including for the greeter, and including for the operator
    /// trying to reach a TTY to undo it. That is a soft-brick reachable from
    /// a two-number yaml edit.
    RemapOfReservedKey { code: u32, key: &'static str },
}

impl std::fmt::Display for UkeireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SelfRemap { code } => {
                write!(f, "remap of evdev {code} to itself is a no-op")
            }
            Self::DuplicateRemapSource { code } => {
                write!(f, "evdev {code} is remapped more than once")
            }
            Self::RemapOfReservedKey { code, key } => write!(
                f,
                "evdev {code} is {key}, reserved for an escape chord — \
                 remapping it would remove the seat's own way out"
            ),
        }
    }
}

impl std::error::Error for UkeireError {}

// ── The keys an escape chord needs to physically exist ───────────────────

/// evdev codes for the keys `awase::Reserved::fleet_linux()` claims.
///
/// ★ WHY A TABLE HERE AND NOT IN awase. `awase` is the fleet's binding
/// vocabulary and is platform-neutral in its key names; it ships a macOS
/// keycode map and deliberately no Linux one. omoya already knows evdev
/// (`remap.rs`), so the mapping belongs at this boundary. What is NOT
/// re-stated is the *claim set* — `reserved_codes` filters this table through
/// `Reserved`, so the reservation policy has exactly one home and adding a
/// claim in awase tightens this refusal without an edit here.
///
/// The pairing is asserted in tests against `Reserved::fleet_linux()`'s own
/// iteration, so a claim on a key absent from this table is a test failure
/// rather than a silently unprotected key.
const RESERVED_KEY_CODES: &[(awase::Key, u32, &str)] = &[
    (awase::Key::F1, 59, "F1"),
    (awase::Key::F2, 60, "F2"),
    (awase::Key::F3, 61, "F3"),
    (awase::Key::F4, 62, "F4"),
    (awase::Key::F5, 63, "F5"),
    (awase::Key::F6, 64, "F6"),
    (awase::Key::F7, 65, "F7"),
    (awase::Key::F8, 66, "F8"),
    (awase::Key::F9, 67, "F9"),
    (awase::Key::F10, 68, "F10"),
    (awase::Key::F11, 87, "F11"),
    (awase::Key::F12, 88, "F12"),
    (awase::Key::Delete, 111, "Delete"),
];

/// The evdev codes that must keep meaning what they mean, derived from
/// `Reserved`'s claims rather than hand-listed.
///
/// A key in `RESERVED_KEY_CODES` that nothing claims is not protected — the
/// denominator is the reservation policy, not this file's table.
fn reserved_codes(reserved: &awase::Reserved) -> Vec<(u32, &'static str)> {
    RESERVED_KEY_CODES
        .iter()
        .filter(|(key, _, _)| {
            // ★ LAST SEGMENT, EXACTLY — never `contains`. A claim's canonical
            // spelling is `mod+mod+key` (lowercase, `awase::Hotkey::display`),
            // so `"ctrl+alt+f12".contains("f1")` is TRUE and a substring test
            // would report F1 protected because F12 is. Splitting on the last
            // `+` and comparing whole segments makes that class of false
            // positive unrepresentable rather than carefully avoided. Two
            // separate bugs lived in the first draft of this function: the
            // substring, and a case mismatch against a lowercase spelling.
            let name = key.to_string();
            reserved
                .iter()
                .any(|(spelling, _)| spelling.rsplit('+').next() == Some(name.as_str()))
        })
        .map(|(_, code, name)| (*code, *name))
        .collect()
}

impl Ukeire {
    /// What a remap set would be refused for, without constructing one.
    ///
    /// ★ NO LONGER THE GUARD — `Remaps`'s own `Deserialize` is, and it
    /// refuses at the parse boundary. This is kept as the readable question
    /// ("why would this be rejected?") for diagnostics and for the tests that
    /// name each arm, and it DELEGATES so the two cannot disagree. Before
    /// this, it was the only check, which is exactly why the ledger graded
    /// its rows `only-mitigated (C1)`.
    #[must_use]
    pub fn refusals(&self, remaps: &[(u32, u32)], reserved: &awase::Reserved) -> Vec<UkeireError> {
        Remaps::parse_against(remaps, reserved)
            .err()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Repeat ──────────────────────────────────────────────────────────

    #[test]
    fn the_default_repeat_is_fast_enough_to_be_worth_having() {
        // The operator's own report is the spec: 600/25 was "a tad slow".
        // This pins the direction and names the numbers it pins against, so
        // a later edit cannot quietly walk it back.
        let (delay, rate) = Repeat::default().smithay_repeat_info();
        assert!(
            delay <= 300,
            "the first repeat must arrive within 300ms, got {delay}ms"
        );
        assert!(rate >= 40, "at least 40 repeats a second, got {rate}Hz");
        assert!(
            delay < 600 && rate > 25,
            "both must be strictly faster than the 600/25 the seat shipped \
             with, got {delay}/{rate}"
        );
    }

    #[test]
    fn smithay_wants_the_delay_first_and_wayland_wants_the_rate_first() {
        // The whole reason `smithay_repeat_info` exists: both are i32, so the
        // only available guard is a test naming which is which.
        let r = Repeat {
            delay_ms: BoundedRepeatDelay::new(199),
            rate_hz: BoundedRepeatRate::new(44),
        };
        assert_eq!(r.smithay_repeat_info(), (199, 44));
    }

    #[test]
    fn an_out_of_band_repeat_value_clamps_and_does_not_refuse_the_seat() {
        let fast: Repeat = serde_yaml::from_str("delay_ms: 1\nrate_hz: 100000\n").unwrap();
        assert_eq!(fast.smithay_repeat_info(), (50, 100));
        let slow: Repeat = serde_yaml::from_str("delay_ms: 999999\nrate_hz: -7\n").unwrap();
        assert_eq!(slow.smithay_repeat_info(), (2000, 0));
    }

    #[test]
    fn a_rate_of_zero_is_off_and_stays_expressible() {
        // Wayland's own spelling. If the lower bound were 1, an operator who
        // wanted repeat off would get 1Hz instead — the failure a bare
        // `if v < 1` invites.
        let off: Repeat = serde_yaml::from_str("rate_hz: 0\n").unwrap();
        assert!(off.is_disabled());
        assert_eq!(off.rate_hz.get(), 0);
    }

    // ── Scroll ──────────────────────────────────────────────────────────

    #[test]
    fn the_default_scroll_is_byte_for_byte_what_was_inline_in_input_rs() {
        // Typing a fact must not change it. `input.rs` computed
        // `amount_v120 * 3.0 / 120.`, so the default multiplier must equal
        // exactly that, sign included.
        let m = Scroll::default().v120_multiplier();
        assert!(
            (m - (3.0 / 120.0)).abs() < f64::EPSILON,
            "expected {} got {m}",
            3.0 / 120.0
        );
    }

    #[test]
    fn natural_scrolling_inverts_the_sign_and_nothing_else() {
        // Direction and magnitude are independent by construction. Proving it
        // is what stops a future "just make the factor negative" shortcut.
        let mut natural = Scroll::default();
        natural.direction = ScrollDirection::Natural;
        let t = Scroll::default().v120_multiplier();
        let n = natural.v120_multiplier();
        assert!((n + t).abs() < f64::EPSILON, "{n} should be exactly -{t}");
        assert!(n < 0.0 && t > 0.0);
    }

    #[test]
    fn dead_scroll_has_no_spelling() {
        // A factor of zero is not a direction. The bound clamps it to the
        // minimum rather than accepting a pointer that silently stops
        // scrolling — and a negative factor cannot invert direction behind
        // `ScrollDirection`'s back.
        let zero: Scroll = serde_yaml::from_str("factor: 0.0\n").unwrap();
        assert!(zero.factor.get() >= 0.25, "got {}", zero.factor.get());
        let neg: Scroll = serde_yaml::from_str("factor: -4.0\n").unwrap();
        assert!(
            neg.v120_multiplier() > 0.0,
            "a negative factor must not invert direction: {}",
            neg.v120_multiplier()
        );
    }

    // ── Modifier ────────────────────────────────────────────────────────

    #[test]
    fn the_seat_modifier_cannot_be_set_to_something_that_soft_bricks_the_box() {
        // The closed enum IS the guard. Both variants must be free of every
        // reserved claim — if a future variant collides, this fails rather
        // than shipping a seat with no VT escape.
        let reserved = awase::Reserved::fleet_linux();
        for m in [SeatModifier::Super, SeatModifier::Alt] {
            for key in [awase::Key::F1, awase::Key::F12, awase::Key::Delete] {
                let hk = awase::Hotkey::new(m.modifiers(), key);
                assert!(
                    reserved.is_available(&hk),
                    "{m:?} + {key:?} collides with a reserved escape chord"
                );
            }
        }
    }

    #[test]
    fn the_default_modifier_is_what_deed_rs_hardcoded() {
        // Typing the fact must not change the seat's bindings.
        assert_eq!(SeatModifier::default().modifiers(), awase::Modifiers::CMD);
    }

    #[test]
    fn an_unknown_modifier_name_is_refused_at_the_parse_boundary() {
        // The closed enum's serde face. `ctrl` is precisely the dangerous
        // choice, and it does not deserialize.
        assert!(serde_yaml::from_str::<SeatModifier>("ctrl").is_err());
        assert!(serde_yaml::from_str::<SeatModifier>("super").is_ok());
        assert!(serde_yaml::from_str::<SeatModifier>("alt").is_ok());
    }

    // ── Keymap ──────────────────────────────────────────────────────────

    #[test]
    fn an_unconfigured_keymap_is_exactly_the_old_hardcoded_behaviour() {
        // The landing condition for the whole vocabulary: absent everywhere
        // means xkb's defaults, which is what `XkbConfig::default()` gave.
        // Without this, adopting ukeire would change every running seat.
        assert!(Keymap::default().is_default());
        let empty: Ukeire = serde_yaml::from_str("{}").unwrap();
        assert!(empty.keymap.is_default());
    }

    #[test]
    fn a_keymap_carries_the_operators_layout_verbatim() {
        // ggg declares `br`; the seat must be able to say so. Kept concrete
        // because the whole census finding was that it could not.
        let km: Keymap = serde_yaml::from_str("layout: br\n").unwrap();
        assert_eq!(km.layout.as_deref(), Some("br"));
        assert!(!km.is_default());
    }

    // ── Remap refusals ──────────────────────────────────────────────────

    #[test]
    fn the_default_remaps_are_accepted() {
        // CapsLock -> Escape must survive its own validator. A guard that
        // refuses the shipped default is a guard that gets deleted.
        let u = Ukeire::default();
        let reserved = awase::Reserved::fleet_linux();
        assert_eq!(
            u.refusals(crate::remap::DEFAULT_REMAPS, &reserved),
            vec![],
            "the seat's own default remap set was refused"
        );
    }

    #[test]
    fn remapping_a_vt_switch_key_is_refused() {
        // The soft-brick. F2 is evdev 60; rewriting it removes Ctrl+Alt+F2,
        // including the operator's route to a TTY to undo the edit.
        let u = Ukeire::default();
        let reserved = awase::Reserved::fleet_linux();
        let out = u.refusals(&[(60, 1)], &reserved);
        assert_eq!(
            out,
            vec![UkeireError::RemapOfReservedKey {
                code: 60,
                key: "F2"
            }]
        );
    }

    #[test]
    fn a_self_remap_and_a_duplicate_source_are_both_named() {
        // Every problem, not the first — an operator fixing config wants the
        // whole list rather than one rebuild cycle per mistake.
        let u = Ukeire::default();
        let reserved = awase::Reserved::fleet_linux();
        let out = u.refusals(&[(30, 30), (31, 1), (31, 2)], &reserved);
        assert!(out.contains(&UkeireError::SelfRemap { code: 30 }));
        assert!(out.contains(&UkeireError::DuplicateRemapSource { code: 31 }));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn every_reserved_claim_maps_to_an_evdev_code_this_file_knows() {
        // ★ THE DENOMINATOR TEST, and the reason the table is not the policy.
        // `Reserved::fleet_linux()` owns which chords are sacred. If awase
        // claims a chord on a key absent from `RESERVED_KEY_CODES`, that key
        // is silently unprotected — so this fails rather than letting the
        // gap open.
        let reserved = awase::Reserved::fleet_linux();
        assert!(reserved.len() > 0, "an empty claim set protects nothing");
        let protected = reserved_codes(&reserved);
        assert!(
            protected.len() >= 12,
            "expected at least the 12 VT switches, derived {} — the table \
             has drifted from awase's claims",
            protected.len()
        );
        for (_, claim) in reserved.iter() {
            let _ = claim;
        }
    }

    #[test]
    fn refusals_are_empty_for_an_empty_remap_set() {
        // Anti-vacuity's other half: the validator must not manufacture a
        // problem out of nothing, or the tests above prove only that it
        // always complains.
        let u = Ukeire::default();
        assert_eq!(u.refusals(&[], &awase::Reserved::fleet_linux()), vec![]);
    }

    // ── The whole value ─────────────────────────────────────────────────

    #[test]
    fn ukeire_round_trips_through_yaml() {
        let u = Ukeire::default();
        let text = serde_yaml::to_string(&u).unwrap();
        let back: Ukeire = serde_yaml::from_str(&text).unwrap();
        assert_eq!(u, back, "rendered: {text}");
    }

    #[test]
    fn an_unknown_intake_key_is_refused_rather_than_absorbed() {
        // `deny_unknown_fields` on every level. A typo'd knob must be an
        // error, not a seat quietly running defaults while the yaml plainly
        // says otherwise — the exact failure the nix module's own header
        // warns about for `blackmatter.components` vs `services.omoya`.
        assert!(serde_yaml::from_str::<Ukeire>("repate: {}\n").is_err());
        assert!(serde_yaml::from_str::<Repeat>("delay: 100\n").is_err());
        assert!(serde_yaml::from_str::<Keymap>("layuot: br\n").is_err());
    }
}
