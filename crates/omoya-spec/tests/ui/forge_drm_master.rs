//! ILLEGAL: conjuring a second DRM master. DRM master is exclusive per card —
//! a world-fact — so the type must not be constructible outside the crate.
//! Expected: an unnumbered "cannot construct ... due to private fields".
use omoya_spec::DrmMaster;

fn main() {
    let _second = DrmMaster {};
}
