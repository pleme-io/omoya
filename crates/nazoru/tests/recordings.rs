mod rig;

use nazoru::{Acceleration, Action, BASE_PX_PER_MM, TouchpadConfig, code};
use rig::{buttons, motion_raw_total, motions, replay, scrolls, stops, zek_pad, zek_pad_with};

const CONTROL_STROKE: &str = include_str!("recordings/control_stroke.evemu");
const TAP_ONE: &str = include_str!("recordings/tap_one.evemu");
const TAP_TWO: &str = include_str!("recordings/tap_two.evemu");
const SCROLL_TWO: &str = include_str!("recordings/scroll_two.evemu");
const CLICK_LEFT: &str = include_str!("recordings/click_left.evemu");
const CLICK_BOTTOM_RIGHT: &str = include_str!("recordings/click_bottom_right.evemu");
const CLICK_TWO_FINGER: &str = include_str!("recordings/click_two_finger.evemu");
const PALM: &str = include_str!("recordings/palm_lands_while_pointing.evemu");
const THUMB: &str = include_str!("recordings/thumb_rests_then_index_points.evemu");

const LEFT: u32 = code::BTN_LEFT as u32;
const RIGHT: u32 = code::BTN_RIGHT as u32;

fn flat() -> TouchpadConfig {
    TouchpadConfig {
        acceleration: Acceleration::Flat,
        ..TouchpadConfig::default()
    }
}

#[test]
fn an_empty_recording_produces_nothing() {
    assert_eq!(replay(&mut zek_pad(), ""), vec![]);
}

#[test]
fn reports_with_no_contact_produce_nothing() {
    let quiet =
        "E: 0.000000 0000 0000 0000\nE: 0.008000 0004 0005 8000\nE: 0.008000 0000 0000 0000\n";
    assert_eq!(replay(&mut zek_pad(), quiet), vec![]);
}

#[test]
fn the_control_stroke_moves_the_pointer_and_does_nothing_else() {
    let actions = replay(&mut zek_pad_with(flat()), CONTROL_STROKE);
    assert_eq!(buttons(&actions), vec![], "a stroke must not click");
    assert_eq!(scrolls(&actions), vec![], "one finger must not scroll");
    assert_eq!(stops(&actions), 0);
    assert_eq!(motions(&actions).len(), 3, "the first frame has zero delta");
    let total = motion_raw_total(&actions);
    assert!(
        (total.0 - 15.0 * BASE_PX_PER_MM).abs() < 1e-9,
        "240 units at 16 units/mm is 15 mm: {total:?}"
    );
    assert!(total.1.abs() < 1e-9);
}

#[test]
fn a_one_finger_tap_is_a_left_click() {
    let actions = replay(&mut zek_pad(), TAP_ONE);
    assert_eq!(
        buttons(&actions),
        vec![(LEFT, true, 96_000), (LEFT, false, 96_000)]
    );
    assert_eq!(scrolls(&actions), vec![]);
}

#[test]
fn a_two_finger_tap_is_a_right_click() {
    let actions = replay(&mut zek_pad(), TAP_TWO);
    assert_eq!(
        buttons(&actions),
        vec![(RIGHT, true, 96_000), (RIGHT, false, 96_000)]
    );
    assert_eq!(scrolls(&actions), vec![]);
    assert_eq!(motions(&actions), vec![], "nobody moved");
}

#[test]
fn two_fingers_scroll_naturally_by_default_and_stop_when_one_lifts() {
    let actions = replay(&mut zek_pad(), SCROLL_TWO);
    let scrolled = scrolls(&actions);
    assert_eq!(scrolled.len(), 4, "{actions:?}");
    let total: f64 = scrolled.iter().map(|s| s.1).sum();
    let detents = 12.0 / nazoru::MM_PER_DETENT;
    assert!(
        (total + detents * nazoru::V120_PER_DETENT).abs() < 1e-9,
        "fingers moved down 12 mm, so natural scroll is negative: {total}"
    );
    assert!(scrolled.iter().all(|s| s.0 == 0.0), "no sideways scroll");
    assert!(scrolled.iter().all(|s| s.2), "marked natural");
    assert_eq!(stops(&actions), 1, "a finger source owes one axis_stop");
    assert_eq!(buttons(&actions), vec![], "a scroll must not tap");
    assert_eq!(
        motions(&actions),
        vec![],
        "a scroll must not move the pointer"
    );
    assert!(actions.iter().any(|a| matches!(
        a,
        Action::ScrollStop {
            horizontal: false,
            vertical: true,
            time_us: 40_000
        }
    )));
}

#[test]
fn a_physical_click_is_left() {
    let actions = replay(&mut zek_pad(), CLICK_LEFT);
    assert_eq!(
        buttons(&actions),
        vec![(LEFT, true, 40_000), (LEFT, false, 120_000)]
    );
}

#[test]
fn a_click_in_the_bottom_right_is_right_and_its_release_matches_after_the_finger_leaves() {
    let actions = replay(&mut zek_pad(), CLICK_BOTTOM_RIGHT);
    assert_eq!(
        buttons(&actions),
        vec![(RIGHT, true, 40_000), (RIGHT, false, 160_000)]
    );
    assert_eq!(motions(&actions).len(), 2, "the held finger drags");
}

#[test]
fn a_two_finger_click_is_right() {
    let actions = replay(&mut zek_pad(), CLICK_TWO_FINGER);
    assert_eq!(
        buttons(&actions),
        vec![(RIGHT, true, 60_000), (RIGHT, false, 140_000)]
    );
}

#[test]
fn a_palm_landing_while_pointing_neither_jumps_nor_scrolls() {
    let actions = replay(&mut zek_pad_with(flat()), PALM);
    assert_eq!(motions(&actions).len(), 4, "{actions:?}");
    let total = motion_raw_total(&actions);
    assert!(
        (total.0 - 10.0 * BASE_PX_PER_MM).abs() < 1e-9,
        "160 units of finger travel is 10 mm: {total:?}"
    );
    assert!(total.1.abs() < 1e-9);
    assert_eq!(scrolls(&actions), vec![]);
    assert_eq!(buttons(&actions), vec![]);
}

#[test]
fn a_resting_thumb_lets_the_index_finger_point_with_no_jump_or_click() {
    let actions = replay(&mut zek_pad_with(flat()), THUMB);
    assert_eq!(motions(&actions).len(), 3, "{actions:?}");
    let total = motion_raw_total(&actions);
    assert!(
        (total.0 - 15.0 * BASE_PX_PER_MM).abs() < 1e-9,
        "only the index finger's 240 units count: {total:?}"
    );
    assert!(total.1.abs() < 1e-9);
    assert_eq!(scrolls(&actions), vec![]);
    assert_eq!(buttons(&actions), vec![]);
}

#[test]
fn tapping_off_keeps_the_rest_of_the_machine_and_drops_only_the_tap() {
    let config = TouchpadConfig {
        tap_to_click: false,
        ..flat()
    };
    assert_eq!(
        replay(&mut zek_pad_with(config), TAP_ONE).len(),
        1,
        "jitter"
    );
    assert_eq!(buttons(&replay(&mut zek_pad_with(config), TAP_ONE)), vec![]);
    assert_eq!(buttons(&replay(&mut zek_pad_with(config), TAP_TWO)), vec![]);
    assert_eq!(
        buttons(&replay(&mut zek_pad_with(config), CLICK_LEFT)).len(),
        2,
        "a physical click still works"
    );
    assert_eq!(
        motions(&replay(&mut zek_pad_with(config), CONTROL_STROKE)).len(),
        3
    );
}
