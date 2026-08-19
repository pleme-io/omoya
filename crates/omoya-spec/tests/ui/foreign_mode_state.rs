//! ILLEGAL: a fourth mode. `ModeState` is sealed, so a downstream crate cannot
//! add one and silently make every `match` in omoya non-exhaustive.
//! Expected: E0277 — the sealed supertrait is not satisfied.
use omoya_spec::ModeState;

struct Kiosk;

impl ModeState for Kiosk {
    const NAME: &'static str = "kiosk";
}

fn main() {}
