//! ILLEGAL: one authentication authorizing two transitions.
//! Expected: E0382 — `AuthProof` is not `Clone` and was moved.
use omoya_spec::{Compositor, CompositorEnv, Entrance, MockEnv};

fn main() {
    let mut env = MockEnv::new().accepting("luis", "grapheme");
    let entrance = Compositor::<Entrance>::start(&mut env).unwrap();
    let proof = env.authenticate("luis", "grapheme").unwrap();

    let locked = entrance.enter_session(proof, &mut env).lock(&mut env);
    // `proof` was spent above. Spending it again is the bug this prevents.
    let _session = locked.unlock(proof, &mut env);
}
