//! omoya's typed configuration surface.
//!
//! ── ★ WHY THIS EXISTS ─────────────────────────────────────────────────────
//! The operator's rule, stated directly: *"all desktop software should be ours
//! and therefore fully shikumi configuration compatible."* omoya was the one
//! piece of the seat with **no configuration surface at all** — a hand-rolled
//! `std::env::args()` loop, and every other operator-visible value a Rust
//! `const`. A non-US operator could not use this seat, and no key could be
//! rebound, without a recompile.
//!
//! This is the fleet's shape, not a bespoke one: `shikumi::TieredConfig` plus
//! `ConfigDiscovery`, exactly as mado does it, so an operator who has learned
//! one pleme-io config has learned this one.
//!
//! ── ★ THE TIERS, AND WHAT EACH IS FOR ─────────────────────────────────────
//! shikumi's three tiers are not decoration; they answer different questions.
//!
//! | tier | question |
//! |---|---|
//! | `bare` | what does omoya do with NOTHING configured? |
//! | `discovered` | what can be learned from the machine? |
//! | `prescribed_default` | what do we think an operator should get? |
//!
//! `bare` matters here more than in most tools: this is the compositor, and a
//! config that fails to parse must still yield a seat someone can log into and
//! fix the config from. So `bare` is deliberately a *working* seat, not an
//! empty struct.
//!
//! ── ★ WHAT IS DELIBERATELY **NOT** HERE ───────────────────────────────────
//! **The palette.** omoya's colours derive from `irodori::NORD` through
//! `theme.rs`, which states that it carries no hex value of its own. A
//! per-machine colour override would be a Pente violation — the fleet's visual
//! spine is one authored source, and a seat that can drift from it is the
//! defect, not the feature. `bar.rs` already records what one band index
//! escaping to a call site cost: nord9 read as the accent for months at 1.35:1
//! against the real one, *"so it never read as a mistake, only as a duller
//! desktop."*
//!
//! **Keybindings.** They belong in tatara-lisp, not YAML — a keymap is an
//! ordered list of typed records, which is what `(defbind …)` is and what a
//! YAML map is not (ordering lost, duplicate-key behaviour parser-defined).
//! `awase` already owns the fleet binding vocabulary and two independent
//! `BindSpec` derivations already exist in `frost-lisp` and `nami-core` — a
//! third one authored here would be the third copy, which the convergence rule
//! says to extract rather than write. `pending-omoya-lisp-bindings`.
//!
//! **Derived values.** `border = gap / 2`, and the loop that binds eight
//! `(Key, Direction)` pairs across two modifier sets. tatara-lisp has no
//! computation at the compile layer and YAML has none either; Nix does, and
//! the module trio is already the rendering surface.

use serde::{Deserialize, Serialize};

/// The whole operator-facing surface of the seat.
///
/// ★ Every field here was previously a CLI flag or a Rust `const`. Flags still
/// work and still WIN — see [`OmoyaConfig::with_cli_overrides`] — because a
/// compositor that could only be configured by a file you cannot open without
/// a compositor would be a trap.
// No `Eq` — it contains `PlacementConfig`, which contains floats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OmoyaConfig {
    /// What `Logo+Return` launches.
    pub terminal: Option<Vec<String>>,
    /// What `Ctrl+Space` launches.
    pub launcher: Option<Vec<String>>,
    /// Keycode remaps applied above xkb, as `(from_evdev, to_evdev)`.
    pub remaps: Vec<Remap>,
    /// The status bar.
    pub bar: BarConfig,
    /// Window placement rules.
    pub placement: PlacementConfig,
    /// How windows are arranged — tiling or floating, and how floating
    /// windows snap and cascade.
    pub layout: LayoutConfig,
    /// How the seat decides what changed on screen.
    pub damage: DamageConfig,
    /// Held-key repeat.
    pub keyboard: KeyboardConfig,
}

/// How the seat decides what changed on screen.
///
/// ── ★ WHY THIS IS TYPED CONFIG AND NOT AN ENV VAR ────────────────────────
/// It was an env var — `OMOYA_TRUEDAMAGE`, read once by
/// `truedamage::Mode::from_env` — which made the single most safety-relevant
/// knob in the compositor the one thing outside its own config. The seat could
/// not answer "what damage policy am I running" from `omoya config-show`, and
/// the fleet's CONFIGURATION MANAGEMENT rule says every operator-facing knob
/// resolves through shikumi's tiers. Now it does, and the env var survives as
/// an OVERRIDE (see `Mode::resolve`) because A/B-ing a live seat without a
/// rebuild is exactly what an escape hatch is for.
///
/// ── ★ THE EXCLUSIVITY NEEDS NO CHECK ─────────────────────────────────────
/// `authority` is a closed enum, so "off and verify at once" has no
/// representation — truly-unrepresentable, not validated-at-startup. That
/// distinction is the whole reason this is an enum and not three booleans:
/// three booleans would need a cross-field rule, and a cross-field rule on the
/// thing that draws the login screen is a rule that can refuse you a seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DamageConfig {
    /// How much authority the compositor's own shadow diff has over what the
    /// client declared: `off` computes nothing, `on` replaces the declaration,
    /// `verify` computes and publishes the counters but throws the answer away
    /// — an honest A/B whose pixels are identical to `off`.
    pub authority: crate::truedamage::Mode,
    /// How the composed shadow reaches the scanout mapping.
    pub flush: FlushPolicy,
}

/// How the composed shadow reaches the scanout mapping.
///
/// ── ★ WHY THIS IS A KNOB AND NOT SIMPLY FIXED ────────────────────────────
/// The full copy is 8 294 400 bytes into write-combining memory on every
/// presented frame. Measured on plo 2026-09-02 with `flush_us` — which exists
/// because this number had never been separable from `frame_us` — it is
/// **3 729 µs of a 3 756 µs frame, 99.3%**, against a 2 778 µs vblank
/// interval at 360 Hz. Under a typing burst: 7 722 of 7 858 µs. The seat is
/// bound almost entirely by this one memcpy, and nothing upstream of it — no
/// change detector, no sketch, no predictor — can reach that term.
///
/// ── ★ WHY IT IS NOT SIMPLY FLIPPED ───────────────────────────────────────
/// The partial copy was DISABLED on 2026-08-30 for cause: the scanout held
/// stale content the shadow did not, so some damage term under-reported, and
/// `nuri_renderer.rs`'s own header records that *which* term was never
/// identified. Re-opening it on a hunch is how the defect shipped the first
/// time.
///
/// It is a knob rather than a flip because the evidence now points one way and
/// is not yet conclusive. `stale_scan` renders the same scene twice in one
/// frame — once at the natural buffer age, once at age 0 — and compares, so it
/// measures under-reporting directly and independently of this policy. On
/// 2026-09-02, after the shm import was re-keyed from `wl_buffer` to surface,
/// eight typing bursts scored **0 stale pixels, 8/8 clean**, against a 6/8
/// baseline. That is a named, fixed under-reporting term and a green
/// instrument — but the instrument proves the DAMAGE SET is sound, not that
/// every consumer of it is, so the operator opts in rather than inherits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlushPolicy {
    /// Copy the whole shadow every presented frame. Today's behaviour, and
    /// the default — a policy that cannot under-copy, at the price measured
    /// above.
    #[default]
    Full,
    /// Copy only what `render_output` drew, which is already unioned across
    /// this buffer's age.
    ///
    /// ★ REFUSES RATHER THAN GUESSES. The copy runs only when
    /// `mekuri::kentou::Target<Known>::load_preserving` accepts the damage's
    /// baseline; a `Coverage::StaleBaseline` or `OutOfBounds` falls back to a
    /// full copy for that frame. So the failure direction is "copied more than
    /// needed", never "left a stale pixel" — the same one-sided-error
    /// discipline the row-span refinement obeys.
    Baselined,
}

/// How windows are arranged by default.
///
/// ── ★ A MODE, NOT A PER-APP RULE ─────────────────────────────────────────
/// `PlacementConfig::floating_app_ids` already answers "should THIS window
/// float", and it answers it well for a launcher. It cannot answer "I want a
/// floating desktop", because that is a property of the seat rather than of
/// any app — expressing it that way would mean listing every app_id the
/// operator will ever run, and silently tiling the one they forgot.
///
/// So the mode is its own field, and the per-app list keeps working
/// underneath it: in `Tiling` a listed app still floats, and in `Floating`
/// everything floats and the list is simply redundant rather than ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum LayoutMode {
    /// Windows fill the usable zone, splitting it between them. The default,
    /// because it is what the seat has always done and a mode change should
    /// be asked for rather than arrive with an upgrade.
    #[default]
    Tiling,
    /// Windows keep their own size and position, cascade as they open, and
    /// snap to the zone's edges. Nothing is resized to make room.
    Floating,
}

/// How the seat arranges windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutConfig {
    /// `tiling` or `floating`.
    pub mode: LayoutMode,
    /// How close, in logical pixels, a floating window's edge must come to
    /// the usable zone's edge before it is pulled flush with it.
    ///
    /// ★ A THRESHOLD, NOT A TOGGLE. Snapping that always applies is just
    /// maximising; snapping that never applies leaves a one-pixel seam the
    /// operator cannot close by hand. The threshold is what makes it feel
    /// like alignment rather than like the compositor arguing.
    ///
    /// `0` disables snapping without needing a second field to mean "off" —
    /// a distance of zero is exactly "only when already flush", which is a
    /// no-op, so the disabled state is expressible in the same type.
    pub snap_threshold: i32,
    /// How far each successive floating window is offset from the last, so
    /// that opening three terminals does not stack three identical rectangles
    /// in the exact centre with only the top one reachable.
    pub cascade_step: i32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            mode: LayoutMode::Tiling,
            // 16 px: large enough to catch a deliberate nudge toward an edge,
            // small enough that a window parked 20 px away stays there. On the
            // seat's 4 px grid (see docs/SHITSURAI.md) this is 4 grid units.
            snap_threshold: 16,
            // 24 px — six grid units. Enough that a title area and a border
            // are both visible on the window beneath, which is the whole job.
            cascade_step: 24,
        }
    }
}

/// One keycode remap.
///
/// ★ Typed as a struct rather than a `(u32, u32)` tuple so the YAML reads
/// `from: 58` / `to: 1` instead of `[58, 1]`. A two-element array in a config
/// file is a coin-flip about which end is which, and the failure — a keyboard
/// where one key is wrong — is discovered by typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remap {
    /// evdev code to rewrite, e.g. 58 for `KEY_CAPSLOCK`.
    pub from: u32,
    /// evdev code to rewrite it to, e.g. 1 for `KEY_ESC`.
    pub to: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    /// Height in logical pixels. The tiler subtracts this from the usable
    /// zone, so it is load-bearing for layout rather than cosmetic.
    pub height: i32,
    /// Show the clock.
    pub clock: bool,
}

// No `Eq`: `float_width`/`float_height` are `f64`, and floats have no total
// equality. `PartialEq` is what the tests need and is the honest bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlacementConfig {
    /// `app_id`s that float centred instead of joining the tiling tree.
    pub floating_app_ids: Vec<String>,
    /// A floating window's size, as a fraction of the usable area.
    pub float_width: f64,
    pub float_height: f64,
}

/// Held-key repeat: how long before a held key starts repeating, and how
/// fast it repeats once it does.
///
/// ── ★ WHY THE FIELDS ARE `Refined` AND NOT BARE `i32` ────────────────────
/// These two numbers go straight to `wl_keyboard.repeat_info`, and both ends
/// of both ranges are hazards rather than merely odd. A 1 ms delay makes
/// every keystroke a burst; a 5000 ms delay reads as "repeat is broken"; a
/// 500 Hz rate outruns any client's ability to drain its event queue and is
/// indistinguishable from the runaway class `awase::KeyRepeatGate` exists to
/// stop. So the bound is carried by the FIELD TYPE: `Refined`'s `Deserialize`
/// clamps at the parse boundary, which means no expression anywhere in this
/// crate can hold an out-of-band value — there is no constructor that
/// produces one. Per the fleet's input-resilience rule, this is
/// `Refined<T, Bounds>` from `ishou-tokens` rather than a fresh `if v > MAX`.
///
/// Tier honesty: the illegal value is **unrepresentable in this struct**,
/// reached by clamping the operator's yaml rather than refusing it. A
/// compositor that refused to start over a typo'd repeat rate would strand
/// the operator at a blank screen — the same reasoning `bare()` gives.
///
/// ── ★ `rate_hz = 0` IS "OFF", AND THAT IS WAYLAND'S SPELLING, NOT OURS ───
/// `wl_keyboard.repeat_info` defines a rate of zero as "repeat disabled", so
/// the lower bound is 0 and no separate `enable` bool exists. A bool would
/// create the combination *disabled with a delay*, which has no meaning and
/// would need a cross-field rule to reject.
// No `Eq`: `Refined` derives `PartialEq` but not `Eq` — it is generic over a
// `T` that may be a float, so a total equality would be a promise the type
// cannot keep for every instantiation. `PartialEq` is what the tests need and
// is the honest bound, exactly as for `PlacementConfig` above.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardConfig {
    /// Milliseconds a key must be held before the first repeat.
    pub repeat_delay_ms: BoundedRepeatDelay,
    /// Repeats per second once repeating. `0` disables repeat entirely.
    pub repeat_rate_hz: BoundedRepeatRate,
}

/// 50..=2000 ms. Below 50 ms every keypress becomes a burst; above 2 s the
/// operator concludes repeat does not work.
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

/// 0..=100 Hz. `0` is Wayland's "off"; 100 is already twice a fast desktop
/// default and past it the client, not the seat, becomes the bottleneck.
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

pub type BoundedRepeatDelay = ishou_tokens::Refined<i32, RepeatDelayBounds>;
pub type BoundedRepeatRate = ishou_tokens::Refined<i32, RepeatRateBounds>;

/// 200 ms — a deliberate choice, and a change from the 600 ms this seat
/// shipped with.
///
/// 600/25 was chosen when omoya's only face was the greeter, where a held key
/// repeating into a password field is a hazard. That reasoning was sound for
/// that face and wrong for this one: plo now runs a full session, the operator
/// reported the seat "a tad slow", and a session's cursor-key and backspace
/// repeat is a comfort knob, not a security one. The greeter hazard is still
/// real and still unaddressed here — its answer is `awase::KeyRepeatGate` on
/// the entrance face, which bounds repeat at the CONSUMER, so it does not
/// depend on the seat staying slow for everyone.
pub const DEFAULT_REPEAT_DELAY_MS: i32 = 200;

/// 45 Hz — near the fast end of what desktops ship (GNOME tops out ~30,
/// a tuned `xset r rate` is commonly 40-50) without reaching the range where
/// a client cannot drain its queue.
pub const DEFAULT_REPEAT_RATE_HZ: i32 = 45;

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            repeat_delay_ms: BoundedRepeatDelay::new(DEFAULT_REPEAT_DELAY_MS),
            repeat_rate_hz: BoundedRepeatRate::new(DEFAULT_REPEAT_RATE_HZ),
        }
    }
}

impl KeyboardConfig {
    /// The pair `smithay`'s `add_keyboard` / `change_repeat_info` wants, in
    /// its argument order: `(delay_ms, rate_hz)`.
    ///
    /// ★ The order is the trap. smithay takes **delay first**, while
    /// `wl_keyboard.repeat_info` sends **rate first** — swapping them yields
    /// a seat that waits 45 ms and then repeats 200 times a second, which
    /// reads as "the keyboard is possessed" rather than as a config error.
    #[must_use]
    pub fn smithay_repeat_info(self) -> (i32, i32) {
        (self.repeat_delay_ms.get(), self.repeat_rate_hz.get())
    }
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            height: crate::bar::HEIGHT,
            clock: true,
        }
    }
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            floating_app_ids: crate::placement::FLOATING_APP_IDS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            float_width: 0.46,
            float_height: 0.52,
        }
    }
}

impl Default for OmoyaConfig {
    fn default() -> Self {
        Self::prescribed()
    }
}

impl OmoyaConfig {
    /// **Tier 0 — bare.** What omoya does with nothing configured.
    ///
    /// ★ A WORKING SEAT, not an empty struct, and that is the whole point of
    /// this tier existing for a compositor. If a config fails to parse, the
    /// operator needs a seat they can log into and fix it from — a bare tier
    /// that produced no terminal and no remaps would strand them at a blank
    /// screen with no way in.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            terminal: None,
            launcher: None,
            // CapsLock -> Escape survives even the bare tier. It is the seat's
            // default for the reason `remap.rs` gives — the worst-placed key on
            // the board, under the strongest finger — and an operator who has
            // stopped reaching for Escape should not lose it to a typo in a
            // yaml file.
            remaps: crate::remap::DEFAULT_REMAPS
                .iter()
                .map(|(from, to)| Remap {
                    from: *from,
                    to: *to,
                })
                .collect(),
            bar: BarConfig::default(),
            placement: PlacementConfig::default(),
            layout: LayoutConfig::default(),
            // `Mode`'s `Default` is `On`, which is what `from_env` already
            // fell back to for an absent variable — so the bare tier and the
            // pre-config behaviour agree by construction rather than by two
            // constants that could drift.
            damage: DamageConfig::default(),
            keyboard: KeyboardConfig::default(),
        }
    }

    /// **Tier 1 — discovered.** What the machine can tell us.
    ///
    /// Nothing today, and that is stated rather than left implicit: the two
    /// things worth discovering are the display mode — which is read from DRM
    /// at startup, not from config, because a mode this file disagreed with
    /// would be a black screen — and the font path, which currently comes from
    /// an `/etc/fonts` scan that `docs/VISUAL-PERFORMANCE.md` says to replace
    /// with Nix-supplied paths. When that lands, it lands here.
    #[must_use]
    pub fn discovered() -> Self {
        Self::bare()
    }

    /// **Tier 2 — prescribed.** What we think an operator should get.
    #[must_use]
    pub fn prescribed() -> Self {
        Self::bare()
    }

    /// Let explicit CLI flags win over the file.
    ///
    /// ★ **FLAGS BEAT THE FILE, ALWAYS.** This is a compositor: if a bad
    /// config file could not be overridden from the command line, recovering
    /// from one would mean editing yaml without a working seat to edit it in.
    /// `greetd` launches omoya with a command line, so a flag is the escape
    /// hatch that always exists.
    ///
    /// Only `Some` wins — an absent flag leaves the file's value alone rather
    /// than clobbering it with a default, which is the difference between an
    /// override and a reset.
    #[must_use]
    pub fn with_cli_overrides(
        mut self,
        terminal: Option<Vec<String>>,
        launcher: Option<Vec<String>>,
    ) -> Self {
        if terminal.is_some() {
            self.terminal = terminal;
        }
        if launcher.is_some() {
            self.launcher = launcher;
        }
        self
    }

    /// The remaps as `remap::apply` wants them.
    #[must_use]
    pub fn remap_pairs(&self) -> Vec<(u32, u32)> {
        self.remaps.iter().map(|r| (r.from, r.to)).collect()
    }
}

impl shikumi::TieredConfig for OmoyaConfig {
    fn bare() -> Self {
        Self::bare()
    }
    fn discovered() -> Self {
        Self::discovered()
    }
    fn prescribed_default() -> Self {
        Self::prescribed()
    }
}

/// The env var naming an explicit config path.
pub const DISCOVERY_VAR: &str = "OMOYA_CONFIG";

/// The prefix under which individual FIELDS may be overridden by env.
///
/// ★ It must not be a prefix of [`DISCOVERY_VAR`] — see the note in [`load`]
/// and the `the_discovery_var_is_not_a_field_override` test, which is the
/// seal rather than the comment.
pub const FIELD_ENV_PREFIX: &str = "OMOYA_OPT_";

/// Load the seat's config through shikumi's discovery chain.
///
/// ★ **A BROKEN CONFIG MUST NOT COST YOU THE SEAT.** Every failure here
/// returns the bare tier and logs why. The alternative — refusing to start —
/// means a typo in a yaml file leaves a machine with no way in that does not
/// involve another computer. That trade is right for a CLI and wrong for the
/// thing that draws the login screen.
///
/// The warning is deliberately at `warn` and names the path: a seat that
/// silently ignored its config would be worse than one that refused it, since
/// the operator would spend the evening editing a file nothing reads.
#[must_use]
pub fn load() -> OmoyaConfig {
    let path = match shikumi::ConfigDiscovery::new("omoya")
        .env_override(DISCOVERY_VAR)
        .discover()
    {
        Ok(p) => p,
        Err(_) => {
            tracing::info!("no omoya config found — running the prescribed defaults");
            return OmoyaConfig::prescribed();
        }
    };
    // ── ★ THE FIELD-OVERRIDE PREFIX MUST NOT CONTAIN THE DISCOVERY VAR ──
    // `OMOYA_OPT_`, not `OMOYA_`. shikumi's env layer maps `<PREFIX><FIELD>`
    // onto fields, so with the prefix `OMOYA_` the discovery variable
    // `OMOYA_CONFIG` is itself read as a field named `config` -- which does
    // not exist here, so `deny_unknown_fields` refuses the WHOLE load and
    // this falls back to prescribed defaults with a warning.
    //
    // The documented way to point omoya at a config file was therefore the
    // one way to guarantee it ignored the file.
    //
    // Latent since the surface was written: it fires only when someone
    // actually uses the override, and nobody had. Found 2026-08-28 by RUNNING
    // annai (which had copied this idiom) rather than by reading any of the
    // three copies of it.
    match shikumi::ConfigStore::<OmoyaConfig>::load(&path, FIELD_ENV_PREFIX) {
        Ok(store) => {
            tracing::info!(path = %path.display(), "loaded omoya config");
            OmoyaConfig::clone(&store.get())
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "omoya config could not be loaded — falling back to defaults so the seat still comes up"
            );
            OmoyaConfig::prescribed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bare_tier_is_a_WORKING_seat() {
        // ★ THE ONE THAT MATTERS FOR A COMPOSITOR. If a config fails to parse
        // the operator gets this tier, and they need to be able to log in and
        // fix the file. A bare tier with no remaps and no bar would strand
        // them.
        let b = OmoyaConfig::bare();
        assert!(!b.remaps.is_empty(), "bare must keep CapsLock->Escape");
        assert!(b.bar.height > 0, "bare must still draw a bar");
        assert!(
            !b.placement.floating_app_ids.is_empty(),
            "bare must still float the launcher"
        );
    }

    #[test]
    fn the_defaults_match_the_constants_they_replaced() {
        // ★ THE DRIFT GATE. These values existed as Rust `const`s before this
        // file, and the config is only a faithful surface if its defaults ARE
        // those constants. Restating them would let the two disagree, so the
        // defaults are DERIVED from them and this test proves the derivation
        // survives a future edit to either.
        let d = OmoyaConfig::prescribed();
        assert_eq!(d.bar.height, crate::bar::HEIGHT);
        assert_eq!(d.remap_pairs(), crate::remap::DEFAULT_REMAPS.to_vec());
        assert_eq!(
            d.placement.floating_app_ids,
            crate::placement::FLOATING_APP_IDS
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_flag_beats_the_file_but_an_absent_flag_does_not_clobber_it() {
        // ★ The difference between an override and a reset. `greetd` launches
        // omoya with a command line, so a flag is the escape hatch that always
        // exists — but an absent flag must leave the file's value alone, or
        // every unspecified flag would silently erase configuration.
        let from_file = OmoyaConfig {
            terminal: Some(vec!["from-file".into()]),
            launcher: Some(vec!["file-launcher".into()]),
            ..OmoyaConfig::bare()
        };
        let overridden = from_file
            .clone()
            .with_cli_overrides(Some(vec!["from-flag".into()]), None);
        assert_eq!(overridden.terminal, Some(vec!["from-flag".into()]));
        assert_eq!(
            overridden.launcher,
            Some(vec!["file-launcher".into()]),
            "an absent flag must not erase the file's value"
        );
    }

    #[test]
    fn an_unknown_key_is_REFUSED_rather_than_ignored() {
        // ★ `deny_unknown_fields`, and the reason is the failure it prevents:
        // an operator writes `lancher:` for `launcher:`, serde shrugs, and the
        // seat runs the default while the file looks correct. That is the same
        // shape as the tatara-lisp typo trap — a config that reads as applied
        // and is inert.
        let bad = "terminal: [mado]\nlancher: [tobira]\n";
        let r: Result<OmoyaConfig, _> = serde_yaml::from_str(bad);
        assert!(r.is_err(), "a typo'd key must be an error, not a default");

        let good = "terminal: [mado]\nlauncher: [tobira]\n";
        let ok: OmoyaConfig = serde_yaml::from_str(good).expect("the correct spelling parses");
        assert_eq!(ok.launcher, Some(vec!["tobira".to_string()]));
    }

    #[test]
    fn a_partial_file_keeps_every_default_it_did_not_mention() {
        // `#[serde(default)]` on the struct: an operator setting one field must
        // not lose the bar, the remaps, or the float rules.
        let cfg: OmoyaConfig =
            serde_yaml::from_str("terminal: [mado]\n").expect("a one-field file parses");
        assert_eq!(cfg.bar.height, crate::bar::HEIGHT);
        assert_eq!(cfg.remap_pairs(), crate::remap::DEFAULT_REMAPS.to_vec());
    }

    #[test]
    fn the_remap_shape_is_named_not_positional() {
        // A `[58, 1]` pair is a coin-flip about which end is which, and the
        // failure is a keyboard with one wrong key — found by typing.
        let cfg: OmoyaConfig = serde_yaml::from_str("remaps:\n  - from: 58\n    to: 1\n")
            .expect("named remap fields parse");
        assert_eq!(cfg.remap_pairs(), vec![(58, 1)]);
        assert!(
            serde_yaml::from_str::<OmoyaConfig>("remaps:\n  - [58, 1]\n").is_err(),
            "a positional pair must not be accepted"
        );
    }

    #[test]
    fn the_config_round_trips_through_yaml() {
        // If it cannot be written back out, `config-show` cannot exist and an
        // operator has no way to see what they are actually running.
        let a = OmoyaConfig::prescribed();
        let s = serde_yaml::to_string(&a).expect("serializes");
        let b: OmoyaConfig = serde_yaml::from_str(&s).expect("round-trips");
        assert_eq!(a, b);
    }

    #[test]
    fn the_discovery_var_is_not_a_field_override() {
        // THE SEAL for the collision found 2026-08-28. If FIELD_ENV_PREFIX is
        // a prefix of DISCOVERY_VAR, shikumi reads `OMOYA_CONFIG` as a field
        // named `config`; that field does not exist, `deny_unknown_fields`
        // refuses the entire load, and omoya silently falls back to prescribed
        // defaults -- making the documented way to supply a config the one way
        // to guarantee it is ignored.
        //
        // Asserted against the CONSTANTS `load` actually uses, so the two
        // cannot drift apart.
        assert!(
            !DISCOVERY_VAR.starts_with(FIELD_ENV_PREFIX),
            "{DISCOVERY_VAR} is inside the {FIELD_ENV_PREFIX} namespace — \
             the documented config override would disable itself"
        );
    }

    // ── Key repeat ──────────────────────────────────────────────────────

    #[test]
    fn the_default_repeat_is_fast_enough_to_be_worth_having() {
        // The operator's own report is the spec here: 600/25 was "a tad
        // slow". This pins the direction so a future edit cannot quietly
        // walk it back, and states the numbers it is pinning against.
        let (delay, rate) = KeyboardConfig::default().smithay_repeat_info();
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
        // The whole reason `smithay_repeat_info` exists. Passing the pair the
        // wrong way round is not a type error — both are `i32` — so the only
        // available guard is a test that names which is which.
        let cfg = KeyboardConfig {
            repeat_delay_ms: BoundedRepeatDelay::new(199),
            repeat_rate_hz: BoundedRepeatRate::new(44),
        };
        assert_eq!(cfg.smithay_repeat_info(), (199, 44));
    }

    #[test]
    fn an_out_of_band_repeat_value_clamps_and_does_not_refuse_the_seat() {
        // Both fields, both directions. A compositor that refused to start
        // over a typo'd number would strand the operator with no seat to fix
        // it from — `bare()`'s reasoning, applied to a leaf value.
        let fast: KeyboardConfig =
            serde_yaml::from_str("repeat_delay_ms: 1\nrepeat_rate_hz: 100000\n").unwrap();
        assert_eq!(fast.smithay_repeat_info(), (50, 100));

        let slow: KeyboardConfig =
            serde_yaml::from_str("repeat_delay_ms: 999999\nrepeat_rate_hz: -7\n").unwrap();
        assert_eq!(slow.smithay_repeat_info(), (2000, 0));
    }

    #[test]
    fn a_rate_of_zero_is_off_and_stays_expressible() {
        // Wayland's own spelling for "no repeat". If the lower bound were 1,
        // an operator who wanted repeat off would have no way to say so, and
        // would get 1Hz instead — the failure mode a bare `if v < 1` invites.
        let off: KeyboardConfig = serde_yaml::from_str("repeat_rate_hz: 0\n").unwrap();
        assert_eq!(off.repeat_rate_hz.get(), 0);
    }

    #[test]
    fn keyboard_config_round_trips_through_yaml() {
        let cfg = KeyboardConfig::default();
        let text = serde_yaml::to_string(&cfg).unwrap();
        let back: KeyboardConfig = serde_yaml::from_str(&text).unwrap();
        assert_eq!(cfg, back, "rendered: {text}");
    }

    #[test]
    fn every_tier_carries_a_keyboard_and_they_agree() {
        // The bare tier must be a usable seat, so it carries a real pair
        // rather than a zeroed struct.
        for (name, cfg) in [
            ("bare", OmoyaConfig::bare()),
            ("discovered", OmoyaConfig::discovered()),
            ("prescribed", OmoyaConfig::prescribed()),
        ] {
            let (delay, rate) = cfg.keyboard.smithay_repeat_info();
            assert!(delay >= 50, "{name} tier has an unusable delay: {delay}");
            assert!(rate > 0, "{name} tier ships with repeat disabled");
        }
    }
}
