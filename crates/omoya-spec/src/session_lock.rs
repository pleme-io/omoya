#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LockId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requested {
    Granted { locked_now: bool },
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unlocked {
    Opened,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destroyed {
    Quiet,
    InvalidDestroy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Seal {
    #[default]
    Open,
    Held {
        holder: Option<LockId>,
        announced: bool,
        on_glass: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionLock {
    seal: Seal,
    flip_in_flight: bool,
}

impl SessionLock {
    #[must_use]
    pub const fn hides_desktop(&self) -> bool {
        matches!(self.seal, Seal::Held { .. })
    }

    #[must_use]
    pub const fn holder(&self) -> Option<LockId> {
        match self.seal {
            Seal::Held { holder, .. } => holder,
            Seal::Open => None,
        }
    }

    #[must_use]
    pub const fn needs_a_client(&self) -> bool {
        matches!(self.seal, Seal::Held { holder: None, .. })
    }

    #[must_use]
    pub const fn on_glass(&self) -> bool {
        matches!(self.seal, Seal::Held { on_glass: true, .. })
    }

    pub fn compositor_lock(&mut self) -> bool {
        if self.hides_desktop() {
            return false;
        }
        self.seal = Seal::Held {
            holder: None,
            announced: false,
            on_glass: false,
        };
        self.flip_in_flight = false;
        true
    }

    pub fn request(&mut self, id: LockId) -> Requested {
        match self.seal {
            Seal::Open => {
                self.seal = Seal::Held {
                    holder: Some(id),
                    announced: false,
                    on_glass: false,
                };
                self.flip_in_flight = false;
                Requested::Granted { locked_now: false }
            }
            Seal::Held {
                holder: None,
                on_glass,
                ..
            } => {
                self.seal = Seal::Held {
                    holder: Some(id),
                    announced: on_glass,
                    on_glass,
                };
                Requested::Granted {
                    locked_now: on_glass,
                }
            }
            Seal::Held {
                holder: Some(_), ..
            } => Requested::Finished,
        }
    }

    pub fn frame_flipped(&mut self) {
        if self.hides_desktop() {
            self.flip_in_flight = true;
        }
    }

    #[must_use = "the id is the lock that must now be told `locked`"]
    pub fn frame_retired(&mut self) -> Option<LockId> {
        if !self.flip_in_flight {
            return None;
        }
        self.flip_in_flight = false;
        self.on_glass_now()
    }

    #[must_use = "the id is the lock that must now be told `locked`"]
    pub fn on_glass_now(&mut self) -> Option<LockId> {
        let Seal::Held {
            holder, announced, ..
        } = self.seal
        else {
            return None;
        };
        let announce = holder.filter(|_| !announced);
        self.seal = Seal::Held {
            holder,
            announced: announced || holder.is_some(),
            on_glass: true,
        };
        announce
    }

    pub fn unlock(&mut self, id: LockId) -> Unlocked {
        match self.seal {
            Seal::Held {
                holder: Some(holder),
                announced: true,
                ..
            } if holder == id => {
                self.seal = Seal::Open;
                self.flip_in_flight = false;
                Unlocked::Opened
            }
            _ => Unlocked::Refused,
        }
    }

    pub fn destroy(&mut self, id: LockId) -> Destroyed {
        match self.seal {
            Seal::Held {
                holder: Some(holder),
                announced,
                on_glass,
            } if holder == id => {
                self.seal = Seal::Held {
                    holder: None,
                    announced: false,
                    on_glass,
                };
                if announced {
                    Destroyed::InvalidDestroy
                } else {
                    Destroyed::Quiet
                }
            }
            _ => Destroyed::Quiet,
        }
    }

    pub fn holder_gone(&mut self, id: LockId) -> bool {
        match self.seal {
            Seal::Held {
                holder: Some(holder),
                on_glass,
                ..
            } if holder == id => {
                self.seal = Seal::Held {
                    holder: None,
                    announced: false,
                    on_glass,
                };
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: LockId = LockId(1);
    const B: LockId = LockId(2);

    fn locked_by_a() -> SessionLock {
        let mut s = SessionLock::default();
        assert_eq!(s.request(A), Requested::Granted { locked_now: false });
        s.frame_flipped();
        assert_eq!(s.frame_retired(), Some(A));
        s
    }

    #[test]
    fn an_open_seat_shows_the_desktop_and_has_no_holder() {
        let s = SessionLock::default();
        assert!(!s.hides_desktop());
        assert_eq!(s.holder(), None);
        assert!(!s.needs_a_client());
        assert!(!s.on_glass());
    }

    #[test]
    fn a_request_hides_the_desktop_before_any_frame() {
        let mut s = SessionLock::default();
        s.request(A);
        assert!(s.hides_desktop());
        assert_eq!(s.holder(), Some(A));
        assert!(!s.on_glass());
    }

    #[test]
    fn locked_waits_for_a_lock_frame_to_retire_not_to_be_issued() {
        let mut s = SessionLock::default();
        s.request(A);
        assert_eq!(s.frame_retired(), None, "no flip has been issued");
        s.frame_flipped();
        assert!(!s.on_glass(), "a flip issued is not a flip on the glass");
        assert_eq!(s.frame_retired(), Some(A));
        assert!(s.on_glass());
        assert_eq!(s.frame_retired(), None, "locked is said once");
    }

    #[test]
    fn a_flip_issued_before_the_lock_does_not_count_as_a_lock_frame() {
        let mut s = SessionLock::default();
        s.frame_flipped();
        s.request(A);
        assert_eq!(s.frame_retired(), None);
        assert!(!s.on_glass());
    }

    #[test]
    fn a_lock_during_a_pending_flip_waits_for_its_own_frame() {
        let mut s = locked_by_a();
        s.unlock(A);
        s.frame_flipped();
        s.request(B);
        assert_eq!(s.frame_retired(), None);
        s.frame_flipped();
        assert_eq!(s.frame_retired(), Some(B));
    }

    #[test]
    fn a_second_client_is_finished_and_changes_nothing() {
        let mut s = locked_by_a();
        assert_eq!(s.request(B), Requested::Finished);
        assert_eq!(s.holder(), Some(A));
        assert!(s.hides_desktop());
    }

    #[test]
    fn the_bypass_any_object_but_the_announced_holder_cannot_unlock() {
        let mut s = locked_by_a();
        assert_eq!(s.request(B), Requested::Finished);
        assert_eq!(s.unlock(B), Unlocked::Refused);
        assert!(s.hides_desktop());
        assert_eq!(s.holder(), Some(A));
        assert_eq!(s.unlock(LockId(99)), Unlocked::Refused);
        assert!(s.hides_desktop());
    }

    #[test]
    fn a_holder_not_yet_told_locked_cannot_unlock() {
        let mut s = SessionLock::default();
        s.request(A);
        assert_eq!(s.unlock(A), Unlocked::Refused);
        assert!(s.hides_desktop());
    }

    #[test]
    fn the_announced_holder_unlocks_and_the_seat_opens() {
        let mut s = locked_by_a();
        assert_eq!(s.unlock(A), Unlocked::Opened);
        assert!(!s.hides_desktop());
        assert_eq!(s.holder(), None);
        assert!(!s.on_glass());
    }

    #[test]
    fn the_crash_a_dead_holder_leaves_the_desktop_hidden() {
        let mut s = locked_by_a();
        assert!(s.holder_gone(A));
        assert!(s.hides_desktop());
        assert_eq!(s.holder(), None);
        assert!(s.needs_a_client());
        assert!(s.on_glass());
    }

    #[test]
    fn a_dead_client_that_held_nothing_changes_nothing() {
        let mut s = locked_by_a();
        assert!(!s.holder_gone(B));
        assert_eq!(s.holder(), Some(A));
        let mut open = SessionLock::default();
        assert!(!open.holder_gone(A));
        assert!(!open.hides_desktop());
    }

    #[test]
    fn a_new_client_takes_over_a_dead_holder_and_is_told_at_once() {
        let mut s = locked_by_a();
        s.holder_gone(A);
        assert_eq!(s.request(B), Requested::Granted { locked_now: true });
        assert_eq!(s.holder(), Some(B));
        assert_eq!(
            s.unlock(A),
            Unlocked::Refused,
            "the dead holder's id is spent"
        );
        assert_eq!(s.unlock(B), Unlocked::Opened);
    }

    #[test]
    fn a_take_over_before_any_lock_frame_waits_for_one() {
        let mut s = SessionLock::default();
        s.compositor_lock();
        assert_eq!(s.request(A), Requested::Granted { locked_now: false });
        s.frame_flipped();
        assert_eq!(s.frame_retired(), Some(A));
    }

    #[test]
    fn a_compositor_lock_needs_no_client_and_can_reach_the_glass_alone() {
        let mut s = SessionLock::default();
        assert!(s.compositor_lock());
        assert!(s.hides_desktop());
        assert!(s.needs_a_client());
        s.frame_flipped();
        assert_eq!(s.frame_retired(), None, "nobody to tell");
        assert!(s.on_glass());
        assert!(!s.compositor_lock(), "already locked");
    }

    #[test]
    fn a_compositor_lock_leaves_a_live_holder_alone() {
        let mut s = locked_by_a();
        assert!(!s.compositor_lock());
        assert_eq!(s.holder(), Some(A));
    }

    #[test]
    fn nothing_on_the_glass_to_change_still_tells_the_holder() {
        let mut s = SessionLock::default();
        s.request(A);
        assert_eq!(s.on_glass_now(), Some(A));
        assert_eq!(s.on_glass_now(), None);
        assert_eq!(s.unlock(A), Unlocked::Opened);
    }

    #[test]
    fn a_pending_holder_may_cancel_and_the_seat_stays_shut() {
        let mut s = SessionLock::default();
        s.request(A);
        assert_eq!(s.destroy(A), Destroyed::Quiet);
        assert!(s.hides_desktop());
        assert!(s.needs_a_client());
    }

    #[test]
    fn destroy_after_locked_is_a_protocol_error_and_the_seat_stays_shut() {
        let mut s = locked_by_a();
        assert_eq!(s.destroy(A), Destroyed::InvalidDestroy);
        assert!(s.hides_desktop());
        assert!(s.needs_a_client());
    }

    #[test]
    fn a_finished_object_may_destroy_quietly() {
        let mut s = locked_by_a();
        s.request(B);
        assert_eq!(s.destroy(B), Destroyed::Quiet);
        assert_eq!(s.holder(), Some(A));
    }

    #[test]
    fn no_sequence_opens_the_seat_without_the_holders_unlock() {
        let mut seen = 0u32;
        for seq in 0..4096u32 {
            let mut s = SessionLock::default();
            s.compositor_lock();
            let mut v = seq;
            for _ in 0..6 {
                match v % 8 {
                    0 => {
                        s.request(A);
                    }
                    1 => {
                        s.request(B);
                    }
                    2 => s.frame_flipped(),
                    3 => {
                        let _ = s.frame_retired();
                    }
                    4 => {
                        let _ = s.on_glass_now();
                    }
                    5 => {
                        s.holder_gone(A);
                    }
                    6 => {
                        s.destroy(B);
                    }
                    _ => {
                        s.unlock(LockId(7));
                    }
                }
                v /= 8;
                assert!(s.hides_desktop(), "sequence {seq}");
                seen += 1;
            }
        }
        assert_eq!(seen, 4096 * 6);
    }
}
