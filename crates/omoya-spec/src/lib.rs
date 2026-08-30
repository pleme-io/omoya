//! omoya's mode machine — the compositor's `Entrance` / `Session` / `Locked`
//! spine, typed so that the transitions which must not exist have no signature.
//!
//! # Why this crate has no platform dependencies
//!
//! A Wayland compositor is mostly unreachable code on a developer's machine: it
//! needs a seat, a DRM device, a session and root. That is exactly the
//! condition under which a design goes untested until the day it runs on real
//! hardware — and a login manager that is first exercised on the machine it can
//! lock you out of is a bad plan.
//!
//! So the mode machine is separated from everything that needs a kernel. This
//! crate depends on `thiserror` and `tracing`, and on nothing else: no smithay,
//! no `wayland-server`, no `libc`, no `wgpu`. Its invariants are checkable on a
//! Mac, in CI, with no GPU and no seat.
//!
//! `theory/OMOYA.md` M0. The shape is deliberately the same one `mukae-spec`
//! M0 used, because that is the one rung in this family already proven to work.
//!
//! # The illegal states, and the mechanism for each
//!
//! | illegal state | mechanism | tier |
//! |---|---|---|
//! | leaving `Locked` without authenticating | [`Compositor::unlock`] takes an [`AuthProof`] **by value** | truly-unrep (E0061) |
//! | one authentication unlocking twice | `AuthProof` is not `Clone` and is consumed | truly-unrep (E0382) |
//! | forging an `AuthProof` | its only field is private; the only constructor is [`CompositorEnv::authenticate`] | truly-unrep — *"cannot construct with struct literal syntax due to private fields"*, an UNNUMBERED error; the code was predicted as E0063 and measured otherwise |
//! | two DRM masters | [`DrmMaster`] is not `Clone`, has a private field, and every transition **moves** it | truly-unrep (E0382) |
//! | a transition that forgets the DRM master | transitions take `self` by value and return the next state carrying the same master | truly-unrep — there is no path that drops it silently |
//! | a foreign mode state | [`ModeState`] is sealed | truly-unrep (E0277) |
//!
//! # What this crate deliberately does NOT model
//!
//! Compositing, rendering, protocol objects, output topology, input. Those need
//! the platform, and pretending to model them here would produce a mock whose
//! agreement with reality nobody has checked — which is worse than an honest
//! gap. See `theory/OMOYA.md` §5a and M2.

use core::marker::PhantomData;

/// Sealing module — [`ModeState`] may not be implemented outside this crate.
///
/// Without this, a downstream crate could add a fourth mode and every `match`
/// in omoya would silently stop being exhaustive.
mod sealed {
    pub trait Sealed {}
}

/// One of the compositor's three modes, at the type level.
///
/// Sealed on purpose: the set is closed, and closing it is what makes a
/// non-exhaustive match a compile error rather than a runtime surprise.
pub trait ModeState: sealed::Sealed {
    /// The mode's name, for logs and for [`Compositor::mode_name`].
    const NAME: &'static str;
}

/// The greeter. Composites **zero** clients — see `theory/OMOYA.md` §4.1, which
/// rejects the layer-shell-client shape, and §5a, which is why entrance mode
/// needs scanout but never dmabuf import.
pub struct Entrance;

/// The operator's desktop. The only mode that composites foreign clients.
pub struct Session;

/// The lock screen.
///
/// **This is compositor state, not a client**, and that is the whole point:
/// `ext-session-lock-v1`'s load-bearing guarantee is that a dying lock client
/// must not unlock the session. As a mode, exiting requires an [`AuthProof`],
/// so there is no unlock path to lose.
pub struct Locked;

impl sealed::Sealed for Entrance {}
impl sealed::Sealed for Session {}
impl sealed::Sealed for Locked {}
impl ModeState for Entrance {
    const NAME: &'static str = "entrance";
}
impl ModeState for Session {
    const NAME: &'static str = "session";
}
impl ModeState for Locked {
    const NAME: &'static str = "locked";
}

/// Exclusive control of the DRM device.
///
/// DRM master is exclusive per card — a world-fact, not a policy — so this type
/// is move-only and cannot be constructed outside the crate. Every mode
/// transition consumes it and hands it to the next state, which is what makes
/// "a transition that forgot to carry the master" unrepresentable rather than a
/// bug to review for.
///
/// Deliberately **not** `Clone` and **not** `Copy`.
#[derive(Debug)]
pub struct DrmMaster {
    /// Private, which is what makes the type unforgeable from outside. Measured:
    /// an external `DrmMaster {}` literal is the UNNUMBERED "cannot construct
    /// with struct literal syntax due to private fields" — see
    /// `tests/ui/forge_drm_master.stderr`, which is the receipt rather than the
    /// prediction.
    _private: (),
}

/// Evidence that a human authenticated.
///
/// Move-only and unforgeable: the sole constructor is
/// [`CompositorEnv::authenticate`], which is the seam a real implementation
/// puts PAM behind. Consumed by [`Compositor::unlock`] and
/// [`Compositor::enter_session`], so one authentication authorizes exactly one
/// transition.
#[derive(Debug)]
pub struct AuthProof {
    _private: (),
}

/// Everything the mode machine needs from the outside world, behind one
/// mockable seam.
///
/// A real implementation drives PAM and libseat; [`MockEnv`] drives a script.
/// The trait exists so the machine above it is testable with neither.
pub trait CompositorEnv {
    /// Authenticate `user` with `secret`.
    ///
    /// Returns [`AuthProof`] — which only this method can mint, and which the
    /// caller must then *spend* on a transition.
    ///
    /// # Errors
    /// [`ModeError::AuthFailed`] when the credentials are refused.
    fn authenticate(&mut self, user: &str, secret: &str) -> Result<AuthProof, ModeError>;

    /// Acquire DRM master. Called once, at startup.
    ///
    /// # Errors
    /// [`ModeError::NoDrmMaster`] when the device is unavailable or already
    /// mastered by someone else.
    fn acquire_master(&mut self) -> Result<DrmMaster, ModeError>;

    /// Note that a mode transition happened, for logging and for tests.
    fn on_transition(&mut self, from: &'static str, to: &'static str);
}

/// What can go wrong in the mode machine.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModeError {
    /// The credentials were refused.
    #[error("authentication failed")]
    AuthFailed,
    /// DRM master could not be acquired.
    #[error("could not acquire DRM master: {0}")]
    NoDrmMaster(String),
}

/// The compositor, in exactly one mode.
///
/// The mode is a type parameter rather than a field, so a method that only
/// makes sense in one mode is *absent* in the others rather than returning an
/// error. `unlock()` on a `Compositor<Session>` is E0599, not a runtime check.
#[derive(Debug)]
pub struct Compositor<M: ModeState> {
    master: DrmMaster,
    _mode: PhantomData<M>,
}

impl<M: ModeState> Compositor<M> {
    /// This mode's name.
    #[must_use]
    pub fn mode_name(&self) -> &'static str {
        M::NAME
    }

    /// Borrow the DRM master.
    ///
    /// A borrow, never a move: handing it out by value would let a caller strand
    /// the compositor without one.
    #[must_use]
    pub fn master(&self) -> &DrmMaster {
        &self.master
    }

    /// Rebuild in another mode, carrying the master across.
    ///
    /// Private, and the ONLY way any transition is expressed — so every
    /// transition provably preserves the master.
    fn transition<N: ModeState>(self, env: &mut impl CompositorEnv) -> Compositor<N> {
        env.on_transition(M::NAME, N::NAME);
        tracing::info!(from = M::NAME, to = N::NAME, "omoya mode transition");
        Compositor {
            master: self.master,
            _mode: PhantomData,
        }
    }
}

impl Compositor<Entrance> {
    /// Start the compositor in `Entrance` — the only entry point.
    ///
    /// There is no constructor for the other two modes, so a compositor cannot
    /// come up already locked or already in a session.
    ///
    /// # Errors
    /// Propagates [`ModeError::NoDrmMaster`].
    pub fn start(env: &mut impl CompositorEnv) -> Result<Self, ModeError> {
        Ok(Self {
            master: env.acquire_master()?,
            _mode: PhantomData,
        })
    }

    /// Hand the seat to an authenticated user.
    ///
    /// Takes the proof **by value**: one authentication, one session.
    #[must_use]
    pub fn enter_session(
        self,
        proof: AuthProof,
        env: &mut impl CompositorEnv,
    ) -> Compositor<Session> {
        drop(proof);
        self.transition(env)
    }
}

impl Compositor<Session> {
    /// Lock the seat.
    ///
    /// Deliberately needs no proof — locking is always allowed, and requiring
    /// one would make the safe direction the awkward one.
    #[must_use]
    pub fn lock(self, env: &mut impl CompositorEnv) -> Compositor<Locked> {
        self.transition(env)
    }

    /// Return to the greeter — fast user switching.
    #[must_use]
    pub fn to_entrance(self, env: &mut impl CompositorEnv) -> Compositor<Entrance> {
        self.transition(env)
    }
}

impl Compositor<Locked> {
    /// Unlock, which **requires** authenticating.
    ///
    /// ★ The signature is the security property. There is no other method on
    /// `Compositor<Locked>` that yields a `Compositor<Session>`, so "unlock
    /// without a proof" is not a check that could be bypassed — it is a call
    /// that does not compile.
    #[must_use]
    pub fn unlock(self, proof: AuthProof, env: &mut impl CompositorEnv) -> Compositor<Session> {
        drop(proof);
        self.transition(env)
    }
}

/// A scripted [`CompositorEnv`] for tests — no PAM, no DRM, no seat.
#[derive(Debug, Default)]
pub struct MockEnv {
    /// Credentials this mock accepts, as `(user, secret)`.
    accepted: Vec<(String, String)>,
    /// Whether `acquire_master` succeeds.
    master_available: bool,
    /// Every transition, in order — the tape a test asserts against.
    pub transitions: Vec<(&'static str, &'static str)>,
    /// How many times `acquire_master` was called.
    pub master_acquisitions: usize,
}

impl MockEnv {
    /// A mock with a working DRM device and no valid credentials.
    #[must_use]
    pub fn new() -> Self {
        Self {
            master_available: true,
            ..Self::default()
        }
    }

    /// Accept `(user, secret)`.
    #[must_use]
    pub fn accepting(mut self, user: &str, secret: &str) -> Self {
        self.accepted.push((user.to_string(), secret.to_string()));
        self
    }

    /// Make `acquire_master` fail — the "another compositor already has the
    /// card" case.
    #[must_use]
    pub fn without_drm(mut self) -> Self {
        self.master_available = false;
        self
    }

    /// The transition tape as `"from>to"` strings, for compact assertions.
    #[must_use]
    pub fn tape(&self) -> Vec<String> {
        self.transitions
            .iter()
            .map(|(f, t)| format!("{f}>{t}"))
            .collect()
    }
}

impl CompositorEnv for MockEnv {
    fn authenticate(&mut self, user: &str, secret: &str) -> Result<AuthProof, ModeError> {
        if self.accepted.iter().any(|(u, s)| u == user && s == secret) {
            Ok(AuthProof { _private: () })
        } else {
            Err(ModeError::AuthFailed)
        }
    }

    fn acquire_master(&mut self) -> Result<DrmMaster, ModeError> {
        self.master_acquisitions += 1;
        if self.master_available {
            Ok(DrmMaster { _private: () })
        } else {
            Err(ModeError::NoDrmMaster("device busy".into()))
        }
    }

    fn on_transition(&mut self, from: &'static str, to: &'static str) {
        self.transitions.push((from, to));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> MockEnv {
        MockEnv::new().accepting("luis", "grapheme")
    }

    /// M0's done-predicate: a scripted transcript reaches every mode.
    #[test]
    fn the_full_transcript_entrance_session_locked_session() {
        let mut e = env();

        let entrance = Compositor::<Entrance>::start(&mut e).expect("drm available");
        assert_eq!(entrance.mode_name(), "entrance");

        let proof = e.authenticate("luis", "grapheme").expect("valid creds");
        let session = entrance.enter_session(proof, &mut e);
        assert_eq!(session.mode_name(), "session");

        let locked = session.lock(&mut e);
        assert_eq!(locked.mode_name(), "locked");

        let proof = e.authenticate("luis", "grapheme").expect("valid creds");
        let session = locked.unlock(proof, &mut e);
        assert_eq!(session.mode_name(), "session");

        assert_eq!(
            e.tape(),
            vec!["entrance>session", "session>locked", "locked>session"]
        );
    }

    #[test]
    fn bad_credentials_mint_no_proof() {
        let mut e = env();
        assert_eq!(
            e.authenticate("luis", "wrong").unwrap_err(),
            ModeError::AuthFailed
        );
        // And with no proof there is nothing to spend, so the session
        // transition is simply unreachable — see trybuild case `unlock_without_proof`.
    }

    #[test]
    fn a_compositor_cannot_start_without_drm_master() {
        let mut e = MockEnv::new().without_drm();
        let started = Compositor::<Entrance>::start(&mut e);
        assert!(matches!(started, Err(ModeError::NoDrmMaster(_))));
        assert_eq!(e.master_acquisitions, 1);
    }

    /// The master is acquired ONCE and carried across every transition — the
    /// property that makes the parallel-two-VT handoff expressible later.
    #[test]
    fn the_drm_master_is_acquired_once_and_survives_every_transition() {
        let mut e = env();
        let entrance = Compositor::<Entrance>::start(&mut e).unwrap();
        let proof = e.authenticate("luis", "grapheme").unwrap();
        let session = entrance.enter_session(proof, &mut e);
        let locked = session.lock(&mut e);
        let proof = e.authenticate("luis", "grapheme").unwrap();
        let session = locked.unlock(proof, &mut e);

        // Still exactly one acquisition after four transitions.
        assert_eq!(e.master_acquisitions, 1);
        // And it is still reachable.
        let _: &DrmMaster = session.master();
    }

    #[test]
    fn fast_user_switching_returns_to_the_entrance() {
        let mut e = env();
        let entrance = Compositor::<Entrance>::start(&mut e).unwrap();
        let proof = e.authenticate("luis", "grapheme").unwrap();
        let back = entrance.enter_session(proof, &mut e).to_entrance(&mut e);
        assert_eq!(back.mode_name(), "entrance");
        assert_eq!(e.tape(), vec!["entrance>session", "session>entrance"]);
    }

    #[test]
    fn one_authentication_authorizes_exactly_one_transition() {
        let mut e = env();
        let entrance = Compositor::<Entrance>::start(&mut e).unwrap();
        let proof = e.authenticate("luis", "grapheme").unwrap();
        let _session = entrance.enter_session(proof, &mut e);
        // `proof` was moved. Reusing it is E0382 — see the trybuild case
        // `reuse_auth_proof`. A second transition needs a second
        // `authenticate`, which is a second trip through PAM.
        assert_eq!(e.transitions.len(), 1);
    }

    #[test]
    fn mode_names_are_stable_because_a_test_pins_them() {
        // They appear in the transition tape, in logs, and (later) in
        // `omoya --mode <name>`. Renaming one is an interface change.
        assert_eq!(Entrance::NAME, "entrance");
        assert_eq!(Session::NAME, "session");
        assert_eq!(Locked::NAME, "locked");
    }
}
