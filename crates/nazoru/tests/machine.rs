mod rig;

use nazoru::{
    AbsAxis, Acceleration, Action, BASE_PX_PER_MM, BoundedAreaHeight, BoundedAreaWidth,
    BoundedScrollSpeed, BoundedSpeed, BoundedTapMs, BoundedTapTravel, RawEvent, ScrollDirection,
    Surface, Touchpad, TouchpadConfig, axis_value, code,
};
use rig::Step::{Down, Key, Move, Press, Release, Tool, TouchOff, Up};
use rig::{buttons, frame, motion_raw_total, motions, scrolls, stops, zek_pad, zek_pad_with};

const LEFT: u32 = code::BTN_LEFT as u32;
const RIGHT: u32 = code::BTN_RIGHT as u32;

fn flat() -> TouchpadConfig {
    TouchpadConfig {
        acceleration: Acceleration::Flat,
        ..TouchpadConfig::default()
    }
}

#[test]
fn a_new_touch_starts_with_zero_delta_wherever_it_lands() {
    let mut pad = zek_pad_with(flat());
    assert_eq!(frame(&mut pad, 0, &[Down(0, 1, 100, 100)]), vec![]);
    assert_eq!(frame(&mut pad, 200, &[Up(0)]), vec![]);
    let far = frame(&mut pad, 400, &[Down(0, 2, 1800, 1000)]);
    assert_eq!(motions(&far), vec![], "a landing must not jump: {far:?}");
    let first = frame(&mut pad, 408, &[Move(0, 1816, 1000)]);
    assert_eq!(motions(&first), vec![(1.0 * BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_new_touch_in_a_slot_reused_at_the_same_x_keeps_the_kernels_sticky_position() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    frame(&mut pad, 100, &[Up(0)]);
    let mut out = Vec::new();
    pad.feed(
        RawEvent::Abs {
            number: code::ABS_MT_TRACKING_ID,
            value: 2,
        },
        &mut out,
    );
    pad.feed(
        RawEvent::Abs {
            number: code::ABS_MT_POSITION_Y,
            value: 900,
        },
        &mut out,
    );
    pad.feed(RawEvent::Report { time_us: 200_000 }, &mut out);
    assert_eq!(
        out,
        vec![],
        "X was filtered by the kernel and must stay 500"
    );
    let moved = frame(&mut pad, 208, &[Move(0, 516, 900)]);
    assert_eq!(motions(&moved), vec![(1.0 * BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_tracking_id_replaced_in_place_starts_over_with_zero_delta() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 100, 100)]);
    let replaced = frame(&mut pad, 8, &[Down(0, 2, 1500, 300)]);
    assert_eq!(motions(&replaced), vec![], "{replaced:?}");
}

#[test]
fn a_second_finger_landing_does_not_jump_the_pointer() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 400, 400)]);
    frame(&mut pad, 8, &[Move(0, 416, 400)]);
    let landed = frame(&mut pad, 16, &[Down(1, 2, 1700, 400)]);
    assert_eq!(motions(&landed), vec![]);
    assert_eq!(scrolls(&landed), vec![]);
}

#[test]
fn a_position_jump_is_discarded_and_the_next_step_is_measured_from_the_new_place() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 100, 100)]);
    let jumped = frame(&mut pad, 8, &[Move(0, 1700, 100)]);
    assert_eq!(
        motions(&jumped),
        vec![],
        "100 mm in one frame is not a finger"
    );
    let next = frame(&mut pad, 16, &[Move(0, 1716, 100)]);
    assert_eq!(motions(&next), vec![(1.0 * BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_step_of_exactly_the_jump_limit_is_real_motion() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 100, 100)]);
    let step = frame(&mut pad, 8, &[Move(0, 100 + 20 * 16, 100)]);
    assert_eq!(motions(&step), vec![(20.0 * BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_rest_zone_finger_still_counts_at_exactly_the_tap_time() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 700, 1200)]);
    let at_limit = frame(&mut pad, 180, &[Move(0, 716, 1200)]);
    assert_eq!(motions(&at_limit), vec![(BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_thumb_woken_past_the_travel_limit_cannot_tap_when_it_lifts() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 700, 1200)]);
    frame(&mut pad, 300, &[]);
    frame(&mut pad, 308, &[Move(0, 700, 1100)]);
    assert_eq!(buttons(&frame(&mut pad, 316, &[Up(0)])), vec![]);
}

#[test]
fn an_idle_frame_before_a_touch_does_not_age_the_tap() {
    let mut pad = zek_pad();
    assert_eq!(frame(&mut pad, 0, &[]), vec![]);
    frame(&mut pad, 300, &[Down(0, 1, 500, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 380, &[Up(0)])).len(), 2);
}

#[test]
fn a_finger_landing_while_the_button_is_held_cannot_tap() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Press]);
    frame(&mut pad, 8, &[Down(0, 1, 500, 500)]);
    assert_eq!(
        buttons(&frame(&mut pad, 16, &[Release])),
        vec![(LEFT, false, 16_000)]
    );
    assert_eq!(buttons(&frame(&mut pad, 24, &[Up(0)])), vec![]);
}

#[test]
fn the_scroll_slop_accumulates_across_frames() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let first = frame(&mut pad, 8, &[Move(0, 500, 324), Move(1, 900, 324)]);
    assert_eq!(scrolls(&first), vec![], "1.5 mm is inside the slop");
    let second = frame(&mut pad, 16, &[Move(0, 500, 348), Move(1, 900, 348)]);
    assert_eq!(scrolls(&second).len(), 1, "3 mm is past it");
}

#[test]
fn the_scroll_slop_restarts_when_the_finger_count_changes() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 324), Move(1, 900, 324)]);
    frame(&mut pad, 16, &[Down(2, 3, 1300, 300)]);
    frame(&mut pad, 24, &[Up(2)]);
    let moved = frame(&mut pad, 32, &[Move(0, 500, 348), Move(1, 900, 348)]);
    assert_eq!(scrolls(&moved), vec![]);
}

#[test]
fn a_scroll_stop_still_names_an_axis_that_scrolled_twice() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 548, 300), Move(1, 948, 300)]);
    frame(&mut pad, 16, &[Move(0, 596, 300), Move(1, 996, 300)]);
    assert_eq!(
        frame(&mut pad, 24, &[Up(0), Up(1)]),
        vec![Action::ScrollStop {
            horizontal: true,
            vertical: false,
            time_us: 24_000
        }]
    );
}

#[test]
fn the_axis_lock_lets_go_at_exactly_two_to_one_and_holds_past_it() {
    let cases = [
        ((16, 32), (80.0, 160.0)),
        ((32, 16), (160.0, 80.0)),
        ((16, 40), (0.0, 200.0)),
        ((40, 16), (200.0, 0.0)),
    ];
    for ((x, y), expected) in cases {
        let config = TouchpadConfig {
            scroll_direction: ScrollDirection::Traditional,
            ..TouchpadConfig::default()
        };
        let mut pad = zek_pad_with(config);
        frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
        let moved = frame(
            &mut pad,
            8,
            &[Move(0, 500 + x, 300 + y), Move(1, 900 + x, 300 + y)],
        );
        assert_eq!(
            scrolls(&moved),
            vec![(expected.0, expected.1, false)],
            "moved ({x},{y})"
        );
    }
}

#[test]
fn a_finger_turning_into_a_palm_stops_counting_and_coming_back_has_zero_delta() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 400, 400)]);
    frame(&mut pad, 8, &[Move(0, 416, 400)]);
    let palm = frame(
        &mut pad,
        16,
        &[Tool(0, code::MT_TOOL_PALM), Move(0, 500, 400)],
    );
    assert_eq!(motions(&palm), vec![]);
    let back = frame(&mut pad, 24, &[Tool(0, 0), Move(0, 700, 400)]);
    assert_eq!(motions(&back), vec![], "{back:?}");
    let then = frame(&mut pad, 32, &[Move(0, 716, 400)]);
    assert_eq!(motions(&then), vec![(1.0 * BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_thumb_landing_in_the_rest_zone_while_pointing_does_not_become_a_second_finger() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 400, 400)]);
    frame(&mut pad, 8, &[Move(0, 416, 400)]);
    let landed = frame(&mut pad, 16, &[Down(1, 2, 700, 1200), Move(0, 432, 400)]);
    assert_eq!(motions(&landed), vec![(1.0 * BASE_PX_PER_MM, 0.0)]);
    assert_eq!(scrolls(&landed), vec![]);
}

#[test]
fn a_finger_landing_above_the_rest_zone_while_pointing_is_a_second_finger() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 400, 400)]);
    frame(&mut pad, 8, &[Move(0, 416, 400)]);
    let landed = frame(&mut pad, 16, &[Down(1, 2, 700, 1000), Move(0, 432, 400)]);
    assert_eq!(motions(&landed), vec![], "two fingers do not point");
}

#[test]
fn a_rest_zone_finger_that_slides_out_starts_pointing_with_zero_delta() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 700, 1200)]);
    let settled = frame(&mut pad, 300, &[Move(0, 700, 1190)]);
    assert_eq!(motions(&settled), vec![], "still resting");
    let woke = frame(&mut pad, 308, &[Move(0, 700, 1100)]);
    assert_eq!(motions(&woke), vec![], "the frame it wakes has zero delta");
    let moved = frame(&mut pad, 316, &[Move(0, 700, 1084)]);
    assert_eq!(motions(&moved), vec![(0.0, -BASE_PX_PER_MM)]);
}

#[test]
fn a_short_lone_tap_in_the_rest_zone_is_still_a_tap() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 700, 1200)]);
    let lift = frame(&mut pad, 100, &[Up(0)]);
    assert_eq!(
        buttons(&lift),
        vec![(LEFT, true, 100_000), (LEFT, false, 100_000)]
    );
}

#[test]
fn an_index_tap_beside_a_resting_thumb_is_a_left_tap() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 700, 1200)]);
    assert_eq!(buttons(&frame(&mut pad, 300, &[])), vec![]);
    frame(&mut pad, 400, &[Down(1, 2, 1000, 400)]);
    let lift = frame(&mut pad, 480, &[Up(1)]);
    assert_eq!(
        buttons(&lift),
        vec![(LEFT, true, 480_000), (LEFT, false, 480_000)]
    );
    assert_eq!(buttons(&frame(&mut pad, 600, &[Up(0)])), vec![]);
}

#[test]
fn a_tap_with_travel_past_the_limit_does_not_click() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    frame(&mut pad, 40, &[Move(0, 500 + 33, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 80, &[Up(0)])), vec![]);
}

#[test]
fn a_tap_with_travel_at_the_limit_still_clicks() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    frame(&mut pad, 40, &[Move(0, 500 + 32, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 80, &[Up(0)])).len(), 2);
}

#[test]
fn travel_is_the_furthest_reach_so_coming_back_does_not_rescue_a_tap() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    frame(&mut pad, 30, &[Move(0, 600, 500)]);
    frame(&mut pad, 60, &[Move(0, 500, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 90, &[Up(0)])), vec![]);
}

#[test]
fn a_touch_held_past_the_tap_time_does_not_click() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 181, &[Up(0)])), vec![]);
}

#[test]
fn a_touch_lifted_exactly_at_the_tap_time_clicks() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 180, &[Up(0)])).len(), 2);
}

#[test]
fn a_two_finger_tap_held_too_long_does_not_click() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500), Down(1, 2, 900, 500)]);
    frame(&mut pad, 190, &[Up(0)]);
    assert_eq!(buttons(&frame(&mut pad, 200, &[Up(1)])), vec![]);
}

#[test]
fn a_three_finger_tap_does_nothing_yet() {
    let mut pad = zek_pad();
    frame(
        &mut pad,
        0,
        &[
            Down(0, 1, 500, 500),
            Down(1, 2, 900, 500),
            Down(2, 3, 1300, 500),
        ],
    );
    let lift = frame(&mut pad, 80, &[Up(0), Up(1), Up(2)]);
    assert_eq!(buttons(&lift), vec![]);
}

#[test]
fn a_physical_click_does_not_also_tap() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    let press = frame(&mut pad, 30, &[Press]);
    assert_eq!(buttons(&press), vec![(LEFT, true, 30_000)]);
    let release = frame(&mut pad, 60, &[Release]);
    assert_eq!(buttons(&release), vec![(LEFT, false, 60_000)]);
    assert_eq!(buttons(&frame(&mut pad, 90, &[Up(0)])), vec![]);
}

#[test]
fn a_finger_left_down_after_a_release_does_not_start_a_new_tap() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 500), Press]);
    frame(&mut pad, 20, &[Release]);
    assert_eq!(buttons(&frame(&mut pad, 40, &[Up(0)])), vec![]);
}

#[test]
fn a_press_landing_in_the_touching_frame_never_starts_a_tap() {
    let mut pad = zek_pad();
    let press = frame(&mut pad, 0, &[Down(0, 1, 500, 500), Press]);
    assert_eq!(buttons(&press), vec![(LEFT, true, 0)]);
    let release = frame(&mut pad, 40, &[Release, Up(0)]);
    assert_eq!(buttons(&release), vec![(LEFT, false, 40_000)]);
}

#[test]
fn a_press_with_no_contact_is_left_and_a_stray_release_is_silent() {
    let mut pad = zek_pad();
    assert_eq!(buttons(&frame(&mut pad, 0, &[Release])), vec![]);
    assert_eq!(
        buttons(&frame(&mut pad, 8, &[Press])),
        vec![(LEFT, true, 8_000)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 16, &[Press])),
        vec![],
        "already down"
    );
    assert_eq!(
        buttons(&frame(&mut pad, 24, &[Release])),
        vec![(LEFT, false, 24_000)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 32, &[Release])),
        vec![],
        "already up"
    );
}

#[test]
fn the_click_area_is_the_bottom_right_corner_and_nowhere_else() {
    let cases = [
        (1500, 1200, RIGHT),
        (1500, 400, LEFT),
        (400, 1200, LEFT),
        (944, 1037, RIGHT),
        (943, 1037, LEFT),
        (944, 1036, LEFT),
    ];
    for (x, y, expected) in cases {
        let mut pad = zek_pad();
        frame(&mut pad, 0, &[Down(0, 1, x, y)]);
        let press = frame(&mut pad, 30, &[Press]);
        assert_eq!(
            buttons(&press),
            vec![(expected, true, 30_000)],
            "at ({x},{y})"
        );
    }
}

#[test]
fn the_click_area_is_configurable() {
    let config = TouchpadConfig {
        right_click_width_pct: BoundedAreaWidth::new(25.0),
        right_click_height_pct: BoundedAreaHeight::new(10.0),
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 1000, 1100)]);
    assert_eq!(
        buttons(&frame(&mut pad, 30, &[Press])),
        vec![(LEFT, true, 30_000)]
    );
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 1000, 1100)]);
    assert_eq!(
        buttons(&frame(&mut pad, 30, &[Press])),
        vec![(RIGHT, true, 30_000)]
    );
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 1500, 1250)]);
    assert_eq!(
        buttons(&frame(&mut pad, 30, &[Press])),
        vec![(RIGHT, true, 30_000)]
    );
}

#[test]
fn a_palm_in_the_click_area_does_not_make_a_click_right() {
    let mut pad = zek_pad();
    frame(
        &mut pad,
        0,
        &[
            Down(0, 1, 500, 500),
            Down(1, 2, 1500, 1200),
            Tool(1, code::MT_TOOL_PALM),
        ],
    );
    assert_eq!(
        buttons(&frame(&mut pad, 30, &[Press])),
        vec![(LEFT, true, 30_000)]
    );
}

#[test]
fn a_resting_thumb_in_the_click_area_makes_a_click_right() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 1500, 1200)]);
    frame(&mut pad, 300, &[Down(1, 2, 500, 500)]);
    assert_eq!(
        buttons(&frame(&mut pad, 330, &[Press])),
        vec![(RIGHT, true, 330_000)]
    );
}

#[test]
fn a_resting_thumb_outside_the_click_area_leaves_a_click_left() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 400, 1200)]);
    frame(&mut pad, 300, &[Down(1, 2, 500, 500)]);
    assert_eq!(
        buttons(&frame(&mut pad, 330, &[Press])),
        vec![(LEFT, true, 330_000)]
    );
}

#[test]
fn while_a_button_is_held_the_pointer_follows_the_finger_that_moves() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 400, 1200)]);
    frame(&mut pad, 300, &[Down(1, 2, 900, 500), Press]);
    let dragged = frame(&mut pad, 308, &[Move(1, 916, 500)]);
    assert_eq!(motions(&dragged), vec![(1.0 * BASE_PX_PER_MM, 0.0)]);
    assert_eq!(scrolls(&dragged), vec![], "a held button is not a scroll");
    let mut pad = zek_pad_with(flat());
    frame(
        &mut pad,
        0,
        &[Down(0, 1, 400, 500), Down(1, 2, 900, 500), Press],
    );
    let two = frame(&mut pad, 8, &[Move(0, 408, 500), Move(1, 932, 500)]);
    assert_eq!(
        motions(&two),
        vec![(2.0 * BASE_PX_PER_MM, 0.0)],
        "the fastest wins"
    );
}

#[test]
fn a_press_stops_a_scroll_in_progress() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    let press = frame(&mut pad, 16, &[Press]);
    assert_eq!(stops(&press), 1);
    assert_eq!(buttons(&press), vec![(RIGHT, true, 16_000)]);
}

#[test]
fn a_non_clickpad_keeps_its_physical_buttons_as_they_are() {
    let mut pad = Touchpad::new(TouchpadConfig::default(), rig::zek_surface(), false);
    frame(&mut pad, 0, &[Down(0, 1, 1500, 1200)]);
    assert_eq!(
        buttons(&frame(&mut pad, 30, &[Press])),
        vec![(LEFT, true, 30_000)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 60, &[Release])),
        vec![(LEFT, false, 60_000)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 90, &[Key(code::BTN_RIGHT, 1)])),
        vec![(RIGHT, true, 90_000)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 120, &[Key(code::BTN_RIGHT, 0), Up(0)])),
        vec![(RIGHT, false, 120_000)],
        "and a physical press spoils the tap"
    );
}

#[test]
fn a_clickpad_passes_any_other_physical_button_through() {
    let mut pad = zek_pad();
    let middle = u32::from(code::BTN_RIGHT) + 1;
    assert_eq!(
        buttons(&frame(&mut pad, 0, &[Key(0x112, 1)])),
        vec![(middle, true, 0)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 8, &[Key(0x112, 0)])),
        vec![(middle, false, 8_000)]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 16, &[Key(code::BTN_TASK + 1, 1)])),
        vec![]
    );
    assert_eq!(
        buttons(&frame(&mut pad, 24, &[Key(code::BTN_TOOL_FINGER, 1)])),
        vec![]
    );
}

#[test]
fn a_key_autorepeat_on_a_button_is_not_a_second_press() {
    let mut pad = zek_pad();
    assert_eq!(buttons(&frame(&mut pad, 0, &[Key(0x112, 1)])).len(), 1);
    assert_eq!(buttons(&frame(&mut pad, 8, &[Key(0x112, 2)])), vec![]);
}

#[test]
fn a_small_two_finger_wobble_does_not_scroll() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let wobble = frame(&mut pad, 8, &[Move(0, 500, 332), Move(1, 900, 332)]);
    assert_eq!(scrolls(&wobble), vec![], "2 mm is inside the slop");
    let back = frame(&mut pad, 16, &[Move(0, 500, 300), Move(1, 900, 300)]);
    assert_eq!(scrolls(&back), vec![]);
    let out = frame(&mut pad, 24, &[Move(0, 500, 333), Move(1, 900, 333)]);
    assert_eq!(scrolls(&out).len(), 1, "past the slop from the origin");
}

#[test]
fn scrolling_spoils_the_tap_even_if_the_fingers_did_not_reach_the_tap_limit_from_their_start() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 276)]);
    frame(&mut pad, 16, &[Down(1, 2, 900, 300)]);
    let moved = frame(&mut pad, 24, &[Move(0, 500, 320), Move(1, 900, 324)]);
    assert_eq!(scrolls(&moved).len(), 1, "{moved:?}");
    assert_eq!(buttons(&frame(&mut pad, 32, &[Up(0), Up(1)])), vec![]);
}

#[test]
fn traditional_direction_flips_the_sign_and_marks_nothing_natural() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    assert_eq!(scrolls(&scrolled), vec![(0.0, 240.0, false)]);
}

#[test]
fn natural_direction_scrolls_with_the_fingers() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    assert_eq!(scrolls(&scrolled), vec![(-0.0, -240.0, true)]);
}

#[test]
fn one_detent_of_travel_is_120_v120_and_15_axis_units() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        tap_max_travel_mm: BoundedTapTravel::new(0.5),
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 500, 324), Move(1, 900, 324)]);
    assert_eq!(scrolls(&scrolled), vec![(0.0, 120.0, false)]);
    assert!((axis_value(120.0) - 15.0).abs() < f64::EPSILON);
    assert!(
        (axis_value(120.0) / nazoru::MM_PER_DETENT - BASE_PX_PER_MM).abs() < f64::EPSILON,
        "content tracks the fingers at the pointer's base rate"
    );
}

#[test]
fn scroll_speed_scales_scroll_and_nothing_else() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        scroll_speed: BoundedScrollSpeed::new(2.0),
        ..flat()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    assert_eq!(scrolls(&scrolled), vec![(0.0, 480.0, false)]);
    frame(&mut pad, 16, &[Up(0), Up(1)]);
    frame(&mut pad, 200, &[Down(0, 3, 500, 300)]);
    let moved = frame(&mut pad, 208, &[Move(0, 516, 300)]);
    assert_eq!(motions(&moved), vec![(BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_two_finger_scroll_takes_the_average_of_the_fingers() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 500, 396)]);
    assert_eq!(scrolls(&scrolled), vec![(0.0, 240.0, false)]);
}

#[test]
fn a_pinch_does_not_scroll() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let pinch = frame(&mut pad, 8, &[Move(0, 400, 300), Move(1, 1000, 300)]);
    assert_eq!(scrolls(&pinch), vec![]);
    assert_eq!(motions(&pinch), vec![]);
}

#[test]
fn a_vertical_scroll_with_sideways_jitter_does_not_scroll_sideways() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 510, 348), Move(1, 910, 348)]);
    assert_eq!(scrolls(&scrolled), vec![(0.0, 240.0, false)]);
}

#[test]
fn a_horizontal_scroll_with_vertical_jitter_does_not_scroll_vertically_and_stops_only_horizontal() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 548, 310), Move(1, 948, 310)]);
    assert_eq!(scrolls(&scrolled), vec![(240.0, 0.0, false)]);
    let stop = frame(&mut pad, 16, &[Up(0), Up(1)]);
    assert_eq!(
        stop,
        vec![Action::ScrollStop {
            horizontal: true,
            vertical: false,
            time_us: 16_000
        }]
    );
}

#[test]
fn a_true_diagonal_scrolls_both_axes() {
    let config = TouchpadConfig {
        scroll_direction: ScrollDirection::Traditional,
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let scrolled = frame(&mut pad, 8, &[Move(0, 548, 348), Move(1, 948, 348)]);
    assert_eq!(scrolls(&scrolled), vec![(240.0, 240.0, false)]);
}

#[test]
fn a_finger_left_over_after_a_scroll_does_not_move_the_pointer_until_all_lift() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    let lifted = frame(&mut pad, 16, &[Up(1)]);
    assert_eq!(stops(&lifted), 1);
    let tail = frame(&mut pad, 24, &[Move(0, 516, 348)]);
    assert_eq!(motions(&tail), vec![], "{tail:?}");
    frame(&mut pad, 32, &[Up(0)]);
    frame(&mut pad, 300, &[Down(0, 3, 500, 300)]);
    let fresh = frame(&mut pad, 308, &[Move(0, 516, 300)]);
    assert_eq!(motions(&fresh), vec![(BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_two_finger_tap_lifted_one_at_a_time_lets_the_last_finger_point() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 16, &[Up(1)]);
    let moved = frame(&mut pad, 24, &[Move(0, 516, 300)]);
    assert_eq!(
        motions(&moved),
        vec![(BASE_PX_PER_MM, 0.0)],
        "no scroll happened"
    );
}

#[test]
fn a_third_finger_stops_a_scroll_and_nothing_moves_until_all_lift() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    let third = frame(&mut pad, 16, &[Down(2, 3, 1300, 300)]);
    assert_eq!(stops(&third), 1);
    let moved = frame(
        &mut pad,
        24,
        &[Move(0, 516, 348), Move(1, 916, 348), Move(2, 1316, 300)],
    );
    assert_eq!(motions(&moved), vec![]);
    assert_eq!(scrolls(&moved), vec![]);
}

#[test]
fn a_scroll_past_the_slop_does_not_restart_its_slop_while_continuing() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    let small = frame(&mut pad, 16, &[Move(0, 500, 349), Move(1, 900, 349)]);
    assert_eq!(
        scrolls(&small).len(),
        1,
        "a tiny step mid-scroll still scrolls"
    );
}

#[test]
fn an_idle_frame_during_a_scroll_emits_nothing_and_keeps_scrolling() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    assert_eq!(frame(&mut pad, 16, &[]), vec![]);
    assert_eq!(stops(&frame(&mut pad, 24, &[Up(0), Up(1)])), 1);
}

#[test]
fn a_lift_lost_to_a_dropped_report_heals_when_btn_touch_clears() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    let healed = frame(&mut pad, 8, &[Up(0), TouchOff]);
    assert_eq!(healed, vec![], "a lift nobody saw cannot prove a tap");
    frame(&mut pad, 400, &[Down(0, 3, 500, 300)]);
    let moved = frame(&mut pad, 408, &[Move(0, 516, 300)]);
    assert_eq!(
        motions(&moved),
        vec![(BASE_PX_PER_MM, 0.0)],
        "the ghost is gone"
    );
}

#[test]
fn btn_touch_clearing_keeps_a_palm_so_it_can_turn_back_into_a_finger() {
    let mut pad = zek_pad_with(flat());
    frame(
        &mut pad,
        0,
        &[Down(0, 1, 500, 300), Tool(0, code::MT_TOOL_PALM)],
    );
    frame(&mut pad, 8, &[TouchOff]);
    frame(&mut pad, 16, &[Tool(0, 0)]);
    let moved = frame(&mut pad, 24, &[Move(0, 516, 300)]);
    assert_eq!(motions(&moved), vec![(BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn btn_touch_rising_again_in_the_same_frame_does_not_clear() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 300)]);
    frame(&mut pad, 8, &[TouchOff, Key(code::BTN_TOUCH, 1)]);
    let moved = frame(&mut pad, 16, &[Move(0, 516, 300)]);
    assert_eq!(motions(&moved), vec![(BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn a_slot_outside_the_device_is_ignored_and_never_panics() {
    let mut pad = zek_pad();
    assert_eq!(frame(&mut pad, 0, &[Down(99, 1, 500, 500)]), vec![]);
    assert_eq!(frame(&mut pad, 8, &[Down(-1, 2, 500, 500)]), vec![]);
    assert_eq!(frame(&mut pad, 16, &[Up(99)]), vec![]);
    let out_of_bounds = frame(&mut pad, 24, &[Down(5, 3, 500, 500)]);
    assert_eq!(out_of_bounds, vec![], "zek has slots 0..=4");
    assert_eq!(frame(&mut pad, 32, &[Move(5, 600, 500)]), vec![]);
    frame(&mut pad, 40, &[Down(4, 4, 500, 500)]);
    let moved = frame(&mut pad, 48, &[Move(4, 516, 500)]);
    assert_eq!(motions(&moved).len(), 1, "the last real slot still works");
}

#[test]
fn the_slot_starts_where_the_device_said_it_was() {
    let axis = |value, maximum, resolution| AbsAxis {
        value,
        minimum: 0,
        maximum,
        resolution,
    };
    let surface = Surface::new(axis(0, 1888, 16), axis(0, 1296, 16), axis(2, 4, 0)).unwrap();
    let mut pad = Touchpad::new(flat(), surface, true);
    let mut out = Vec::new();
    for (number, value) in [
        (code::ABS_MT_TRACKING_ID, 7),
        (code::ABS_MT_POSITION_X, 500),
        (code::ABS_MT_POSITION_Y, 500),
    ] {
        pad.feed(RawEvent::Abs { number, value }, &mut out);
    }
    pad.feed(RawEvent::Report { time_us: 0 }, &mut out);
    let moved = frame(&mut pad, 8, &[Move(2, 516, 500)]);
    assert_eq!(motions(&moved), vec![(BASE_PX_PER_MM, 0.0)]);
}

#[test]
fn reset_releases_a_held_button_stops_a_scroll_and_forgets_every_finger() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 1500, 1200), Press]);
    let mut out = Vec::new();
    pad.reset(&mut out);
    assert_eq!(buttons(&out), vec![(RIGHT, false, 0)]);
    assert_eq!(
        frame(&mut pad, 8, &[Move(0, 1516, 1200)]),
        vec![],
        "forgotten"
    );

    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300), Down(1, 2, 900, 300)]);
    frame(&mut pad, 8, &[Move(0, 500, 348), Move(1, 900, 348)]);
    let mut out = Vec::new();
    pad.reset(&mut out);
    assert_eq!(stops(&out), 1);
    assert_eq!(buttons(&out), vec![], "no phantom release");
    let mut again = Vec::new();
    pad.reset(&mut again);
    assert_eq!(again, vec![], "a second reset owes nothing");
}

#[test]
fn reset_during_a_tap_owes_no_click_and_the_next_touch_starts_clean() {
    let mut pad = zek_pad_with(flat());
    frame(&mut pad, 0, &[Down(0, 1, 500, 300)]);
    let mut out = Vec::new();
    pad.reset(&mut out);
    assert_eq!(out, vec![]);
    assert_eq!(frame(&mut pad, 8, &[Up(0)]), vec![]);
    frame(&mut pad, 16, &[Down(0, 2, 500, 300)]);
    assert_eq!(buttons(&frame(&mut pad, 32, &[Up(0)])).len(), 2);
}

#[test]
fn reset_clears_half_a_frame_so_it_cannot_land_after_the_session_returns() {
    let mut pad = zek_pad();
    let mut out = Vec::new();
    for event in [
        RawEvent::Key {
            number: code::BTN_LEFT,
            value: 1,
        },
        RawEvent::Key {
            number: 0x112,
            value: 1,
        },
    ] {
        pad.feed(event, &mut out);
    }
    pad.reset(&mut out);
    assert_eq!(frame(&mut pad, 8, &[]), vec![]);
}

#[test]
fn a_clock_that_steps_backwards_never_produces_a_bad_number() {
    let mut pad = zek_pad();
    frame(&mut pad, 1000, &[Down(0, 1, 500, 300)]);
    let moved = frame(&mut pad, 2, &[Move(0, 516, 300)]);
    let (dx, dy) = motions(&moved)[0];
    assert!(dx.is_finite() && dy.is_finite());
    assert!(
        (dx - BASE_PX_PER_MM).abs() < f64::EPSILON,
        "no speed, so no gain: {dx}"
    );
}

#[test]
fn speed_scales_the_pointer_and_raw_stays_unscaled() {
    let config = TouchpadConfig {
        speed: BoundedSpeed::new(2.0),
        ..flat()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 300)]);
    let moved = frame(&mut pad, 8, &[Move(0, 516, 300)]);
    assert_eq!(motions(&moved), vec![(2.0 * BASE_PX_PER_MM, 0.0)]);
    assert_eq!(motion_raw_total(&moved), (BASE_PX_PER_MM, 0.0));
}

#[test]
fn speed_is_physical_so_a_coarser_axis_moves_the_same_for_the_same_millimetres() {
    let axis = |maximum, resolution| AbsAxis {
        value: 0,
        minimum: 0,
        maximum,
        resolution,
    };
    let surface = Surface::new(axis(1180, 10), axis(1620, 20), axis(4, 0)).unwrap();
    let mut pad = Touchpad::new(flat(), surface, true);
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    let moved = frame(&mut pad, 8, &[Move(0, 510, 520)]);
    assert_eq!(motions(&moved), vec![(BASE_PX_PER_MM, BASE_PX_PER_MM)]);
}

#[test]
fn acceleration_adds_gain_only_when_moving_fast() {
    let mut pad = zek_pad();
    frame(&mut pad, 0, &[Down(0, 1, 500, 300)]);
    let slow = frame(&mut pad, 100, &[Move(0, 516, 300)]);
    let (dx, _) = motions(&slow)[0];
    assert!(
        (dx - BASE_PX_PER_MM).abs() < 1e-9,
        "10 mm/s is under the knee: {dx}"
    );
    let fast = frame(&mut pad, 108, &[Move(0, 516 + 64, 300)]);
    let (dx, _) = motions(&fast)[0];
    assert!(
        (dx - 4.0 * BASE_PX_PER_MM * 2.5).abs() < 1e-9,
        "500 mm/s reaches the ceiling: {dx}"
    );
    assert_eq!(
        motion_raw_total(&fast),
        (4.0 * BASE_PX_PER_MM, 0.0),
        "raw has no gain"
    );
}

#[test]
fn tap_timing_and_travel_are_configurable() {
    let config = TouchpadConfig {
        tap_max_ms: BoundedTapMs::new(60),
        tap_max_travel_mm: BoundedTapTravel::new(1.0),
        ..TouchpadConfig::default()
    };
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    assert_eq!(
        buttons(&frame(&mut pad, 100, &[Up(0)])),
        vec![],
        "too slow now"
    );
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    frame(&mut pad, 20, &[Move(0, 520, 500)]);
    assert_eq!(
        buttons(&frame(&mut pad, 40, &[Up(0)])),
        vec![],
        "too far now"
    );
    let mut pad = zek_pad_with(config);
    frame(&mut pad, 0, &[Down(0, 1, 500, 500)]);
    assert_eq!(buttons(&frame(&mut pad, 40, &[Up(0)])).len(), 2);
}

#[test]
fn owned_events_are_absolute_axes_and_button_range_keys_and_nothing_else() {
    assert!(nazoru::owned_by_pad(code::EV_ABS, code::ABS_MT_SLOT));
    assert!(nazoru::owned_by_pad(code::EV_KEY, code::BTN_LEFT));
    assert!(nazoru::owned_by_pad(code::EV_KEY, code::BTN_MISC));
    assert!(!nazoru::owned_by_pad(code::EV_KEY, code::BTN_MISC - 1));
    assert!(!nazoru::owned_by_pad(code::EV_KEY, code::KEY_A));
    assert!(!nazoru::owned_by_pad(code::EV_SYN, code::SYN_REPORT));
    assert!(
        !nazoru::owned_by_pad(0x02, 0x00),
        "REL stays on the transport"
    );
    assert!(
        !nazoru::owned_by_pad(0x04, 0x05),
        "MSC stays on the transport"
    );
}

#[test]
fn only_syn_report_ends_a_frame() {
    assert_eq!(
        RawEvent::from_kernel(code::EV_SYN, code::SYN_REPORT, 0, 5),
        RawEvent::Report { time_us: 5 }
    );
    assert_eq!(
        RawEvent::from_kernel(code::EV_SYN, 3, 0, 5),
        RawEvent::Ignored
    );
    assert_eq!(RawEvent::from_kernel(0x04, 5, 9, 5), RawEvent::Ignored);
    assert_eq!(
        RawEvent::from_kernel(code::EV_KEY, code::BTN_LEFT, 1, 5),
        RawEvent::Key {
            number: code::BTN_LEFT,
            value: 1
        }
    );
    assert_eq!(
        RawEvent::from_kernel(code::EV_ABS, code::ABS_MT_SLOT, 1, 5),
        RawEvent::Abs {
            number: code::ABS_MT_SLOT,
            value: 1
        }
    );
}
