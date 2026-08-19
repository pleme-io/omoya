//! ILLEGAL: leaving `Locked` without authenticating.
//! Expected: E0061 — `unlock` takes an `AuthProof` and one was not supplied.
//!
//! ★ This is the security property of the whole crate, expressed as a call that
//! does not compile rather than a check that could be bypassed.
use omoya_spec::{Compositor, CompositorEnv, Entrance, MockEnv};

fn main() {
    let mut env = MockEnv::new().accepting("luis", "grapheme");
    let entrance = Compositor::<Entrance>::start(&mut env).unwrap();
    let proof = env.authenticate("luis", "grapheme").unwrap();
    let locked = entrance.enter_session(proof, &mut env).lock(&mut env);

    let _session = locked.unlock(&mut env);
}
