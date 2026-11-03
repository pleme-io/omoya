#![allow(dead_code)]

use nazoru::{AbsAxis, Action, Capabilities, RawEvent, Surface, Touchpad, TouchpadConfig, code};

pub fn zek_surface() -> Surface {
    Surface::new(
        AbsAxis {
            value: 0,
            minimum: 0,
            maximum: 1888,
            resolution: 16,
        },
        AbsAxis {
            value: 0,
            minimum: 0,
            maximum: 1296,
            resolution: 16,
        },
        AbsAxis {
            value: 0,
            minimum: 0,
            maximum: 4,
            resolution: 0,
        },
    )
    .expect("zek's measured axes are valid")
}

pub fn zek_pad() -> Touchpad {
    zek_pad_with(TouchpadConfig::default())
}

pub fn zek_pad_with(config: TouchpadConfig) -> Touchpad {
    Touchpad::new(config, zek_surface(), true)
}

pub fn replay(pad: &mut Touchpad, recording: &str) -> Vec<Action> {
    let mut out = Vec::new();
    for line in recording.lines() {
        let Some(body) = line.trim().strip_prefix("E: ") else {
            continue;
        };
        let mut fields = body.split_whitespace();
        let (seconds, micros) = fields
            .next()
            .and_then(|time| time.split_once('.'))
            .expect("evemu time");
        let time_us = seconds.parse::<u64>().expect("seconds") * 1_000_000
            + micros.parse::<u64>().expect("microseconds");
        let kind = u16::from_str_radix(fields.next().expect("type"), 16).expect("hex type");
        let number = u16::from_str_radix(fields.next().expect("code"), 16).expect("hex code");
        let value = fields.next().expect("value").parse::<i32>().expect("value");
        pad.feed(
            RawEvent::from_kernel(kind, number, value, time_us),
            &mut out,
        );
    }
    out
}

pub fn motions(actions: &[Action]) -> Vec<(f64, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Motion { dx, dy, .. } => Some((*dx, *dy)),
            _ => None,
        })
        .collect()
}

pub fn motion_raw_total(actions: &[Action]) -> (f64, f64) {
    actions.iter().fold((0.0, 0.0), |total, a| match a {
        Action::Motion { raw_dx, raw_dy, .. } => (total.0 + raw_dx, total.1 + raw_dy),
        _ => total,
    })
}

pub fn buttons(actions: &[Action]) -> Vec<(u32, bool, u64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Button {
                code,
                pressed,
                time_us,
            } => Some((*code, *pressed, *time_us)),
            _ => None,
        })
        .collect()
}

pub fn scrolls(actions: &[Action]) -> Vec<(f64, f64, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Scroll {
                v120_x,
                v120_y,
                natural,
                ..
            } => Some((*v120_x, *v120_y, *natural)),
            _ => None,
        })
        .collect()
}

pub fn stops(actions: &[Action]) -> usize {
    actions
        .iter()
        .filter(|a| matches!(a, Action::ScrollStop { .. }))
        .count()
}

#[derive(Debug, Clone, Copy)]
pub enum Step {
    Down(i32, i32, i32, i32),
    Move(i32, i32, i32),
    Up(i32),
    Tool(i32, i32),
    Press,
    Release,
    Key(u16, i32),
    TouchOff,
}

pub fn frame(pad: &mut Touchpad, ms: u64, steps: &[Step]) -> Vec<Action> {
    let mut out = Vec::new();
    let abs = |number: u16, value: i32| RawEvent::Abs { number, value };
    for step in steps {
        let events: Vec<RawEvent> = match *step {
            Step::Down(slot, id, x, y) => vec![
                abs(code::ABS_MT_SLOT, slot),
                abs(code::ABS_MT_TRACKING_ID, id),
                abs(code::ABS_MT_POSITION_X, x),
                abs(code::ABS_MT_POSITION_Y, y),
            ],
            Step::Move(slot, x, y) => vec![
                abs(code::ABS_MT_SLOT, slot),
                abs(code::ABS_MT_POSITION_X, x),
                abs(code::ABS_MT_POSITION_Y, y),
            ],
            Step::Up(slot) => vec![
                abs(code::ABS_MT_SLOT, slot),
                abs(code::ABS_MT_TRACKING_ID, -1),
            ],
            Step::Tool(slot, tool) => vec![
                abs(code::ABS_MT_SLOT, slot),
                abs(code::ABS_MT_TOOL_TYPE, tool),
            ],
            Step::Press => vec![RawEvent::Key {
                number: code::BTN_LEFT,
                value: 1,
            }],
            Step::Release => vec![RawEvent::Key {
                number: code::BTN_LEFT,
                value: 0,
            }],
            Step::Key(number, value) => vec![RawEvent::Key { number, value }],
            Step::TouchOff => vec![RawEvent::Key {
                number: code::BTN_TOUCH,
                value: 0,
            }],
        };
        for event in events {
            pad.feed(event, &mut out);
        }
    }
    pad.feed(RawEvent::Report { time_us: ms * 1000 }, &mut out);
    out
}

fn bits(words: &str) -> std::collections::BTreeSet<u16> {
    let mut found = std::collections::BTreeSet::new();
    for (index, word) in words.split_whitespace().rev().enumerate() {
        let value = u64::from_str_radix(word, 16).expect("hex word");
        for bit in 0..64u16 {
            if value & (1u64 << bit) != 0 {
                let base = u16::try_from(index).expect("word index") * 64;
                found.insert(base + bit);
            }
        }
    }
    found
}

pub fn from_proc(prop: &str, key: &str, rel: &str, abs: &str) -> Capabilities {
    Capabilities {
        props: bits(prop),
        keys: bits(key),
        rel: bits(rel),
        abs: bits(abs),
    }
}
