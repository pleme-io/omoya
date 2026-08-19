//! ILLEGAL: minting an `AuthProof` without going through the env seam (i.e.
//! without PAM). Expected: an unnumbered "cannot construct ... due to private
//! fields" — measured, not predicted (E0063 was the guess and it was wrong).
use omoya_spec::AuthProof;

fn main() {
    let _forged = AuthProof {};
}
