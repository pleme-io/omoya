//! A pure-Rust stand-in for the `xkbcommon` crate's API surface.
//!
//! # Why this exists
//!
//! smithay depends on `xkbcommon` unconditionally — it re-exports it in its
//! public API (`pub use xkbcommon::xkb::{self, keysyms, Keycode, Keysym}`) and
//! the dependency carries no `optional = true` and is behind no feature. So no
//! feature choice removes `libxkbcommon.so` from a smithay compositor, and
//! removing it by editing smithay would mean forking a dependency we
//! deliberately keep.
//!
//! Cargo has a seam for exactly this. `[patch.crates-io]` matches on **package
//! name**, so a crate named `xkbcommon` that presents the same API replaces the
//! FFI binding underneath smithay with smithay unmodified. That is what this
//! is. The engine is [`hairetsu`].
//!
//! # What it is not
//!
//! This is **not** an XKB implementation. It does not read
//! `/usr/share/X11/xkb`, it does not parse keymap text, and it serves exactly
//! one layout (`us`). Those limits are enforced, not papered over: an
//! unsupported layout returns `None` so the caller raises its own typed error,
//! rather than silently handing back the wrong keyboard. See
//! [`xkb::Keymap::new_from_names`].

pub mod xkb;
