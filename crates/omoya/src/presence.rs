use crate::owed::Owed;

pub trait SeatDisplay {
    fn pause(&mut self);
    fn activate(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Present,
    Away,
    Returned,
    Refused(String),
}

#[derive(Debug, Default)]
pub struct Presence {
    away: bool,
    returning: bool,
    paused: bool,
}

impl Presence {
    pub fn leave(&mut self) {
        self.away = true;
    }

    pub fn come_back(&mut self) {
        self.away = false;
        self.returning = true;
    }

    #[must_use]
    pub const fn is_away(&self) -> bool {
        self.away
    }

    pub fn settle<D: SeatDisplay>(
        &mut self,
        display: &mut D,
        owed: &mekuri::Gate<Owed>,
    ) -> Settled {
        if self.away {
            self.pause_once(display);
            return Settled::Away;
        }
        if !self.returning {
            return Settled::Present;
        }
        self.pause_once(display);
        match display.activate() {
            Ok(()) => {
                self.paused = false;
                self.returning = false;
                owed.mark(Owed::Resume);
                Settled::Returned
            }
            Err(reason) => {
                self.paused = false;
                Settled::Refused(reason)
            }
        }
    }

    fn pause_once<D: SeatDisplay>(&mut self, display: &mut D) {
        if !self.paused {
            display.pause();
            self.paused = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeDisplay {
        calls: Vec<&'static str>,
        refusals: u32,
    }

    impl SeatDisplay for FakeDisplay {
        fn pause(&mut self) {
            self.calls.push("pause");
        }

        fn activate(&mut self) -> Result<(), String> {
            self.calls.push("activate");
            if self.refusals > 0 {
                self.refusals -= 1;
                return Err(String::from("no master yet"));
            }
            Ok(())
        }
    }

    fn owed_now(gate: &mut mekuri::Gate<Owed>) -> Vec<Owed> {
        match gate.open() {
            mekuri::Verdict::Draw(pass) => {
                let causes = pass.causes();
                let _ = pass.spend(|_| Ok::<(), ()>(()));
                causes
            }
            mekuri::Verdict::Skip => Vec::new(),
        }
    }

    #[test]
    fn a_held_seat_touches_nothing_and_owes_nothing() {
        let mut gate = mekuri::Gate::new();
        let mut display = FakeDisplay::default();
        let mut presence = Presence::default();
        assert_eq!(presence.settle(&mut display, &gate), Settled::Present);
        assert_eq!(presence.settle(&mut display, &gate), Settled::Present);
        assert!(display.calls.is_empty());
        assert!(owed_now(&mut gate).is_empty());
    }

    #[test]
    fn going_away_pauses_once_and_owes_the_vt_nothing() {
        let mut gate = mekuri::Gate::new();
        let mut display = FakeDisplay::default();
        let mut presence = Presence::default();
        presence.leave();
        gate.mark(Owed::Commit);
        for _ in 0..3 {
            assert_eq!(presence.settle(&mut display, &gate), Settled::Away);
        }
        assert_eq!(display.calls, ["pause"]);
        assert!(presence.is_away());
    }

    #[test]
    fn coming_back_activates_and_owes_a_frame_that_keeps_what_was_owed() {
        let mut gate = mekuri::Gate::new();
        let mut display = FakeDisplay::default();
        let mut presence = Presence::default();
        presence.leave();
        gate.mark(Owed::Commit);
        assert_eq!(presence.settle(&mut display, &gate), Settled::Away);

        presence.come_back();
        assert!(!presence.is_away());
        assert_eq!(presence.settle(&mut display, &gate), Settled::Returned);
        assert_eq!(display.calls, ["pause", "activate"]);
        let owed = owed_now(&mut gate);
        assert!(
            owed.contains(&Owed::Resume),
            "a return owes a frame: {owed:?}"
        );
        assert!(
            owed.contains(&Owed::Commit),
            "away kept the ledger: {owed:?}"
        );

        assert_eq!(presence.settle(&mut display, &gate), Settled::Present);
        assert_eq!(display.calls, ["pause", "activate"]);
    }

    #[test]
    fn a_round_trip_between_two_ticks_still_resets_the_device() {
        let gate = mekuri::Gate::new();
        let mut display = FakeDisplay::default();
        let mut presence = Presence::default();
        presence.leave();
        presence.come_back();
        assert_eq!(presence.settle(&mut display, &gate), Settled::Returned);
        assert_eq!(
            display.calls,
            ["pause", "activate"],
            "smithay resets only a device it saw paused"
        );
    }

    #[test]
    fn a_refused_activation_owes_nothing_and_retries_from_a_pause() {
        let mut gate = mekuri::Gate::new();
        let mut display = FakeDisplay {
            refusals: 1,
            ..FakeDisplay::default()
        };
        let mut presence = Presence::default();
        presence.leave();
        assert_eq!(presence.settle(&mut display, &gate), Settled::Away);
        presence.come_back();

        assert_eq!(
            presence.settle(&mut display, &gate),
            Settled::Refused(String::from("no master yet"))
        );
        assert!(owed_now(&mut gate).is_empty());

        assert_eq!(presence.settle(&mut display, &gate), Settled::Returned);
        assert_eq!(display.calls, ["pause", "activate", "pause", "activate"]);
        assert!(owed_now(&mut gate).contains(&Owed::Resume));
    }

    #[test]
    fn the_session_events_and_the_render_loop_are_wired_to_the_gate() {
        let cut = |source: &'static str| {
            source
                .split_once("#[cfg(test)]")
                .map_or(source, |(live, _)| live)
        };
        let main = cut(include_str!("main.rs"));
        for wire in ["presence.leave()", "presence.come_back()"] {
            assert!(main.contains(wire), "main.rs does not call `{wire}`");
        }
        let drm = cut(include_str!("drm.rs"));
        for wire in [
            "impl crate::presence::SeatDisplay for DrmDevice",
            "presence.settle(",
            "presence.is_away()",
            "Settled::Returned",
            "scanout.resume()",
            "flip_pending.store(false",
        ] {
            assert!(drm.contains(wire), "drm.rs does not reach `{wire}`");
        }
    }
}
