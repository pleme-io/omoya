use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IdlePolicy {
    pub blank_after: Option<Duration>,
    pub lock_after: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleStep {
    Blank,
    Lock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panel {
    #[default]
    On,
    Darkening,
    Dark,
}

impl Panel {
    #[must_use]
    pub const fn shows_nothing(self) -> bool {
        !matches!(self, Self::On)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Idle {
    policy: IdlePolicy,
    last_input: Instant,
    blanked: bool,
    locked: bool,
    inhibitors: usize,
}

impl Idle {
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self {
            policy: IdlePolicy {
                blank_after: None,
                lock_after: None,
            },
            last_input: now,
            blanked: false,
            locked: false,
            inhibitors: 0,
        }
    }

    pub fn set_policy(&mut self, policy: IdlePolicy) {
        self.policy = policy;
    }

    #[must_use]
    pub const fn policy(&self) -> IdlePolicy {
        self.policy
    }

    pub fn input(&mut self, now: Instant) {
        self.last_input = now;
        self.blanked = false;
        self.locked = false;
    }

    pub fn inhibit(&mut self) {
        self.inhibitors += 1;
    }

    pub fn release(&mut self, now: Instant) {
        self.inhibitors = self.inhibitors.saturating_sub(1);
        if self.inhibitors == 0 {
            self.last_input = now;
        }
    }

    #[must_use]
    pub const fn inhibited(&self) -> bool {
        self.inhibitors > 0
    }

    #[must_use]
    pub fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_input)
    }

    pub fn tick(&mut self, now: Instant) -> Vec<IdleStep> {
        if self.inhibited() {
            return Vec::new();
        }
        let idle = self.idle_for(now);
        let mut steps = Vec::new();
        if !self.locked && self.policy.lock_after.is_some_and(|after| idle >= after) {
            self.locked = true;
            steps.push(IdleStep::Lock);
        }
        if !self.blanked && self.policy.blank_after.is_some_and(|after| idle >= after) {
            self.blanked = true;
            steps.push(IdleStep::Blank);
        }
        steps
    }

    #[must_use]
    pub fn next_due(&self, now: Instant) -> Option<Duration> {
        if self.inhibited() {
            return None;
        }
        let idle = self.idle_for(now);
        let due = |after: Option<Duration>, done: bool| {
            after.filter(|_| !done).map(|a| a.saturating_sub(idle))
        };
        [
            due(self.policy.lock_after, self.locked),
            due(self.policy.blank_after, self.blanked),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WakeSwallow {
    press: Option<u32>,
}

impl WakeSwallow {
    pub fn woke_with(&mut self, code: u32) {
        self.press = Some(code);
    }

    #[must_use]
    pub const fn pending(&self) -> Option<u32> {
        self.press
    }

    pub fn swallow_release(&mut self, code: u32) -> bool {
        if self.press == Some(code) {
            self.press = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn at() -> (Idle, Instant) {
        let now = Instant::now();
        let mut i = Idle::new(now);
        i.set_policy(IdlePolicy {
            blank_after: Some(300 * S),
            lock_after: Some(600 * S),
        });
        (i, now)
    }

    #[test]
    fn no_policy_never_acts() {
        let now = Instant::now();
        let mut i = Idle::new(now);
        assert_eq!(i.tick(now + 86400 * S), vec![]);
        assert_eq!(i.next_due(now), None);
    }

    #[test]
    fn it_blanks_at_its_time_and_not_before() {
        let (mut i, now) = at();
        assert_eq!(i.tick(now + 299 * S), vec![]);
        assert_eq!(i.tick(now + 300 * S), vec![IdleStep::Blank]);
    }

    #[test]
    fn each_step_fires_once() {
        let (mut i, now) = at();
        assert_eq!(i.tick(now + 300 * S), vec![IdleStep::Blank]);
        assert_eq!(i.tick(now + 301 * S), vec![]);
        assert_eq!(i.tick(now + 600 * S), vec![IdleStep::Lock]);
        assert_eq!(i.tick(now + 601 * S), vec![]);
    }

    #[test]
    fn a_long_gap_locks_before_it_blanks() {
        let (mut i, now) = at();
        assert_eq!(i.tick(now + 900 * S), vec![IdleStep::Lock, IdleStep::Blank]);
    }

    #[test]
    fn input_restarts_the_clock_and_arms_both_steps() {
        let (mut i, now) = at();
        i.tick(now + 900 * S);
        i.input(now + 900 * S);
        assert_eq!(i.tick(now + 901 * S), vec![]);
        assert_eq!(i.tick(now + 1200 * S), vec![IdleStep::Blank]);
    }

    #[test]
    fn an_inhibitor_holds_both_steps_and_release_starts_a_fresh_wait() {
        let (mut i, now) = at();
        i.inhibit();
        assert!(i.inhibited());
        assert_eq!(i.tick(now + 900 * S), vec![]);
        assert_eq!(i.next_due(now + 900 * S), None);
        i.release(now + 900 * S);
        assert!(!i.inhibited());
        assert_eq!(i.tick(now + 901 * S), vec![]);
        assert_eq!(i.tick(now + 1200 * S), vec![IdleStep::Blank]);
    }

    #[test]
    fn two_inhibitors_hold_until_the_last_one_goes() {
        let (mut i, now) = at();
        i.inhibit();
        i.inhibit();
        i.release(now + 900 * S);
        assert!(i.inhibited());
        assert_eq!(i.tick(now + 901 * S), vec![]);
        i.release(now + 901 * S);
        i.release(now + 901 * S);
        assert!(!i.inhibited(), "a stray release must not underflow");
    }

    #[test]
    fn next_due_names_the_nearest_step_left() {
        let (mut i, now) = at();
        assert_eq!(i.next_due(now + 100 * S), Some(200 * S));
        i.tick(now + 300 * S);
        assert_eq!(i.next_due(now + 300 * S), Some(300 * S));
        i.tick(now + 600 * S);
        assert_eq!(i.next_due(now + 600 * S), None);
    }

    #[test]
    fn a_panel_shows_nothing_unless_on() {
        assert!(!Panel::On.shows_nothing());
        assert!(Panel::Darkening.shows_nothing());
        assert!(Panel::Dark.shows_nothing());
        assert_eq!(Panel::default(), Panel::On);
    }

    #[test]
    fn the_press_that_wakes_is_swallowed_with_its_release_and_nothing_else() {
        let mut w = WakeSwallow::default();
        assert!(!w.swallow_release(30));
        w.woke_with(30);
        assert_eq!(w.pending(), Some(30));
        assert!(!w.swallow_release(31));
        assert!(w.swallow_release(30));
        assert!(!w.swallow_release(30));
        assert_eq!(w.pending(), None);
    }
}
