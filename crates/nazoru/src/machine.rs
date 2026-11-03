use std::time::Duration;

use crate::code;
use crate::config::{BoundedScale, ScrollDirection, TouchpadConfig};
use crate::surface::Surface;
use crate::transfer::{BASE_PX_PER_MM, PointerTransfer};

pub const V120_PER_DETENT: f64 = 120.0;
pub const AXIS_VALUE_PER_DETENT: f64 = 15.0;
pub const MM_PER_DETENT: f64 = 1.5;
pub const MAX_JUMP_MM: f64 = 20.0;

#[must_use]
pub fn axis_value(v120: f64) -> f64 {
    v120 * AXIS_VALUE_PER_DETENT / V120_PER_DETENT
}

#[must_use]
pub const fn owned_by_pad(kind: u16, number: u16) -> bool {
    kind == code::EV_ABS || (kind == code::EV_KEY && number >= code::BTN_MISC)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEvent {
    Key { number: u16, value: i32 },
    Abs { number: u16, value: i32 },
    Report { time_us: u64 },
    Ignored,
}

impl RawEvent {
    #[must_use]
    pub const fn from_kernel(kind: u16, number: u16, value: i32, time_us: u64) -> Self {
        match kind {
            code::EV_KEY => Self::Key { number, value },
            code::EV_ABS => Self::Abs { number, value },
            code::EV_SYN if number == code::SYN_REPORT => Self::Report { time_us },
            _ => Self::Ignored,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Motion {
        dx: f64,
        dy: f64,
        raw_dx: f64,
        raw_dy: f64,
        time_us: u64,
    },
    Button {
        code: u32,
        pressed: bool,
        time_us: u64,
    },
    Scroll {
        v120_x: f64,
        v120_y: f64,
        natural: bool,
        time_us: u64,
    },
    ScrollStop {
        horizontal: bool,
        vertical: bool,
        time_us: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rest {
    Never,
    AfterTapTime,
    AtOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Free,
    Scrolling { horizontal: bool, vertical: bool },
    Tail,
}

#[derive(Debug, Clone, Copy)]
struct Contact {
    tracking: i32,
    settled: bool,
    last_mm: (f64, f64),
    start_mm: (f64, f64),
    delta_mm: (f64, f64),
    travel_mm: f64,
    born_us: u64,
    rest: Rest,
    excluded: bool,
}

impl Contact {
    fn fresh(tracking: i32) -> Self {
        Self {
            tracking,
            settled: false,
            last_mm: (0.0, 0.0),
            start_mm: (0.0, 0.0),
            delta_mm: (0.0, 0.0),
            travel_mm: 0.0,
            born_us: 0,
            rest: Rest::Never,
            excluded: false,
        }
    }

    fn settle(&mut self, at: (f64, f64), now: u64, in_rest_zone: bool, others: usize) {
        self.settled = true;
        self.last_mm = at;
        self.start_mm = at;
        self.delta_mm = (0.0, 0.0);
        self.born_us = now;
        self.rest = match (in_rest_zone, others > 0) {
            (false, _) => Rest::Never,
            (true, false) => Rest::AfterTapTime,
            (true, true) => Rest::AtOnce,
        };
    }

    fn advance(&mut self, at: (f64, f64)) {
        let step = (at.0 - self.last_mm.0, at.1 - self.last_mm.1);
        self.last_mm = at;
        let stride = step.0.hypot(step.1);
        if stride > MAX_JUMP_MM {
            self.delta_mm = (0.0, 0.0);
            self.travel_mm = self.travel_mm.max(stride);
            return;
        }
        self.delta_mm = step;
        let reach = (at.0 - self.start_mm.0).hypot(at.1 - self.start_mm.1);
        self.travel_mm = self.travel_mm.max(reach);
    }

    fn resting(&self, now: u64, tap_limit_us: u64, travel_limit: f64) -> bool {
        self.travel_mm <= travel_limit
            && match self.rest {
                Rest::Never => false,
                Rest::AfterTapTime => now.saturating_sub(self.born_us) > tap_limit_us,
                Rest::AtOnce => true,
            }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    x: i32,
    y: i32,
    tool: i32,
    contact: Option<Contact>,
}

#[derive(Debug, Clone, Copy)]
struct Tap {
    started_us: u64,
    peak: usize,
    spoiled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    fingers: usize,
    sum: (f64, f64),
    fastest: (f64, f64),
    travel: f64,
}

impl Tally {
    fn add(&mut self, delta: (f64, f64), travel: f64) {
        self.fingers += 1;
        self.sum.0 += delta.0;
        self.sum.1 += delta.1;
        self.travel = self.travel.max(travel);
        if delta.0.hypot(delta.1) > self.fastest.0.hypot(self.fastest.1) {
            self.fastest = delta;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Touchpad {
    config: TouchpadConfig,
    transfer: PointerTransfer,
    surface: Surface,
    clickpad: bool,
    slots: Vec<Slot>,
    current: Option<usize>,
    last_us: u64,
    left_edge: Option<bool>,
    touch_ended: bool,
    others: Vec<(u32, bool)>,
    held: Option<u32>,
    flow: Flow,
    two_finger_mm: (f64, f64),
    tap: Option<Tap>,
    previous_fingers: usize,
}

impl Touchpad {
    #[must_use]
    pub fn new(config: TouchpadConfig, surface: Surface, clickpad: bool) -> Self {
        Self {
            config,
            transfer: config.transfer(),
            surface,
            clickpad,
            slots: vec![Slot::default(); surface.slots()],
            current: surface.first_slot(),
            last_us: 0,
            left_edge: None,
            touch_ended: false,
            others: Vec::new(),
            held: None,
            flow: Flow::Free,
            two_finger_mm: (0.0, 0.0),
            tap: None,
            previous_fingers: 0,
        }
    }

    pub fn feed(&mut self, event: RawEvent, out: &mut Vec<Action>) {
        match event {
            RawEvent::Key { number, value } => self.key(number, value),
            RawEvent::Abs { number, value } => self.abs(number, value),
            RawEvent::Report { time_us } => self.frame(time_us, out),
            RawEvent::Ignored => {}
        }
    }

    pub fn reset(&mut self, out: &mut Vec<Action>) {
        let now = self.last_us;
        self.stop_scroll(now, out);
        if let Some(code) = self.held.take() {
            out.push(Action::Button {
                code,
                pressed: false,
                time_us: now,
            });
        }
        for slot in &mut self.slots {
            slot.contact = None;
        }
        self.left_edge = None;
        self.touch_ended = false;
        self.others.clear();
        self.two_finger_mm = (0.0, 0.0);
        self.flow = Flow::Free;
        self.tap = None;
        self.previous_fingers = 0;
    }

    fn key(&mut self, number: u16, value: i32) {
        if value == 2 {
            return;
        }
        match number {
            code::BTN_TOUCH => self.touch_ended = value == 0,
            code::BTN_LEFT if self.clickpad => self.left_edge = Some(value != 0),
            n if (code::BTN_LEFT..=code::BTN_TASK).contains(&n) => {
                self.others.push((u32::from(n), value != 0));
            }
            _ => {}
        }
    }

    fn abs(&mut self, number: u16, value: i32) {
        if number == code::ABS_MT_SLOT {
            self.current = usize::try_from(value).ok();
            return;
        }
        let Some(slot) = self.current.and_then(|index| self.slots.get_mut(index)) else {
            return;
        };
        match number {
            code::ABS_MT_POSITION_X => slot.x = value,
            code::ABS_MT_POSITION_Y => slot.y = value,
            code::ABS_MT_TOOL_TYPE => slot.tool = value,
            code::ABS_MT_TRACKING_ID => {
                let same = slot.contact.is_some_and(|c| c.tracking == value);
                if !same {
                    slot.contact = (value >= 0).then(|| Contact::fresh(value));
                }
            }
            _ => {}
        }
    }

    fn frame(&mut self, now: u64, out: &mut Vec<Action>) {
        let dt = Duration::from_micros(now.saturating_sub(self.last_us));
        self.last_us = now;
        if std::mem::take(&mut self.touch_ended) {
            for slot in &mut self.slots {
                if slot.tool != code::MT_TOOL_PALM
                    && slot.contact.take().is_some()
                    && let Some(tap) = self.tap.as_mut()
                {
                    tap.spoiled = true;
                }
            }
        }
        let tally = self.settle_contacts(now);
        let left_edge = self.left_edge.take();
        let pressed = left_edge == Some(true) || self.others.iter().any(|(_, down)| *down);
        let holding = self.held.is_some() || left_edge == Some(true);
        self.steer(&tally, holding, dt, now, out);
        self.buttons(left_edge, &tally, now, out);
        self.tap_step(pressed, &tally, now, out);
        if tally.fingers == 0 {
            self.flow = Flow::Free;
        }
        self.previous_fingers = tally.fingers;
    }

    fn settle_contacts(&mut self, now: u64) -> Tally {
        let others = self.previous_fingers;
        let tap_limit = self.tap_limit_us();
        let travel_limit = self.config.tap_max_travel_mm.get();
        let zone = 1.0 - self.config.right_click_height_pct.get() / 100.0;
        let surface = self.surface;
        let mut tally = Tally::default();
        for slot in &mut self.slots {
            let Some(contact) = slot.contact.as_mut() else {
                continue;
            };
            let at = surface.mm(slot.x, slot.y);
            if contact.settled {
                contact.advance(at);
            } else {
                let in_zone = surface.fraction(slot.x, slot.y).1 >= zone;
                contact.settle(at, now, in_zone, others);
            }
            if slot.tool == code::MT_TOOL_PALM || contact.resting(now, tap_limit, travel_limit) {
                contact.excluded = true;
                continue;
            }
            let delta = if std::mem::take(&mut contact.excluded) {
                (0.0, 0.0)
            } else {
                contact.delta_mm
            };
            tally.add(delta, contact.travel_mm);
        }
        tally
    }

    fn steer(
        &mut self,
        tally: &Tally,
        holding: bool,
        dt: Duration,
        now: u64,
        out: &mut Vec<Action>,
    ) {
        let two = !holding && tally.fingers == 2;
        if !two {
            self.stop_scroll(now, out);
            self.two_finger_mm = (0.0, 0.0);
        }
        if holding {
            self.move_pointer(tally.fastest, dt, now, out);
        } else if two {
            self.scroll((tally.sum.0 / 2.0, tally.sum.1 / 2.0), now, out);
        } else if tally.fingers == 1 && self.flow == Flow::Free {
            self.move_pointer(tally.sum, dt, now, out);
        }
    }

    fn move_pointer(&self, mm: (f64, f64), dt: Duration, now: u64, out: &mut Vec<Action>) {
        if mm == (0.0, 0.0) {
            return;
        }
        let (dx, dy) = self.transfer.apply(mm, dt);
        let unaccelerated = PointerTransfer::Flat {
            scale: BoundedScale::new(BASE_PX_PER_MM),
        };
        let raw = unaccelerated.apply(mm, dt);
        out.push(Action::Motion {
            dx,
            dy,
            raw_dx: raw.0,
            raw_dy: raw.1,
            time_us: now,
        });
    }

    fn scroll(&mut self, mm: (f64, f64), now: u64, out: &mut Vec<Action>) {
        self.two_finger_mm.0 += mm.0;
        self.two_finger_mm.1 += mm.1;
        if !matches!(self.flow, Flow::Scrolling { .. }) {
            let reach = self.two_finger_mm.0.hypot(self.two_finger_mm.1);
            if reach <= self.config.tap_max_travel_mm.get() {
                return;
            }
            self.flow = Flow::Scrolling {
                horizontal: false,
                vertical: false,
            };
            if let Some(tap) = self.tap.as_mut() {
                tap.spoiled = true;
            }
        }
        let (x, y) = dominant(mm);
        if x == 0.0 && y == 0.0 {
            return;
        }
        if let Flow::Scrolling {
            horizontal,
            vertical,
        } = &mut self.flow
        {
            *horizontal |= x != 0.0;
            *vertical |= y != 0.0;
        }
        let direction = self.config.scroll_direction;
        let per_mm = V120_PER_DETENT * self.config.scroll_speed.get() / MM_PER_DETENT;
        out.push(Action::Scroll {
            v120_x: x * per_mm * direction.sign(),
            v120_y: y * per_mm * direction.sign(),
            natural: direction == ScrollDirection::Natural,
            time_us: now,
        });
    }

    fn stop_scroll(&mut self, now: u64, out: &mut Vec<Action>) {
        if let Flow::Scrolling {
            horizontal,
            vertical,
        } = self.flow
        {
            out.push(Action::ScrollStop {
                horizontal,
                vertical,
                time_us: now,
            });
            self.flow = Flow::Tail;
        }
    }

    fn buttons(&mut self, left_edge: Option<bool>, tally: &Tally, now: u64, out: &mut Vec<Action>) {
        match left_edge {
            Some(true) if self.held.is_none() => {
                let code = self.click_code(tally);
                self.held = Some(code);
                out.push(Action::Button {
                    code,
                    pressed: true,
                    time_us: now,
                });
            }
            Some(false) => {
                if let Some(code) = self.held.take() {
                    out.push(Action::Button {
                        code,
                        pressed: false,
                        time_us: now,
                    });
                }
            }
            _ => {}
        }
        for (code, pressed) in self.others.drain(..) {
            out.push(Action::Button {
                code,
                pressed,
                time_us: now,
            });
        }
    }

    fn click_code(&self, tally: &Tally) -> u32 {
        let right = tally.fingers >= 2 || self.contact_in_right_area();
        u32::from(if right {
            code::BTN_RIGHT
        } else {
            code::BTN_LEFT
        })
    }

    fn contact_in_right_area(&self) -> bool {
        let left_edge = 1.0 - self.config.right_click_width_pct.get() / 100.0;
        let top_edge = 1.0 - self.config.right_click_height_pct.get() / 100.0;
        self.slots.iter().any(|slot| {
            if slot.contact.is_none() || slot.tool == code::MT_TOOL_PALM {
                return false;
            }
            let (x, y) = self.surface.fraction(slot.x, slot.y);
            x >= left_edge && y >= top_edge
        })
    }

    fn tap_step(&mut self, pressed: bool, tally: &Tally, now: u64, out: &mut Vec<Action>) {
        if !self.config.tap_to_click {
            return;
        }
        let travelled = tally.travel > self.config.tap_max_travel_mm.get();
        let limit = self.tap_limit_us();
        if let Some(tap) = self.tap.as_mut() {
            tap.peak = tap.peak.max(tally.fingers);
            tap.spoiled |= pressed || travelled || now.saturating_sub(tap.started_us) > limit;
            if tally.fingers == 0 {
                let finished = *tap;
                self.tap = None;
                if !finished.spoiled {
                    click(finished.peak, now, out);
                }
            }
        } else if self.previous_fingers == 0 && tally.fingers > 0 && self.held.is_none() {
            self.tap = Some(Tap {
                started_us: now,
                peak: tally.fingers,
                spoiled: pressed || travelled,
            });
        }
    }

    fn tap_limit_us(&self) -> u64 {
        u64::try_from(self.config.tap_max_ms.get()).map_or(0, |ms| ms * 1000)
    }
}

fn dominant(mm: (f64, f64)) -> (f64, f64) {
    if mm.0.abs() * 2.0 < mm.1.abs() {
        (0.0, mm.1)
    } else if mm.1.abs() * 2.0 < mm.0.abs() {
        (mm.0, 0.0)
    } else {
        mm
    }
}

fn click(fingers: usize, now: u64, out: &mut Vec<Action>) {
    let number = match fingers {
        1 => code::BTN_LEFT,
        2 => code::BTN_RIGHT,
        _ => return,
    };
    for pressed in [true, false] {
        out.push(Action::Button {
            code: u32::from(number),
            pressed,
            time_us: now,
        });
    }
}
