mod rig;

use nazoru::{Class, code};
use rig::from_proc;

const ZEK_TOUCHPAD_KEY: &str = "e520 10000 0 0 0 0";
const ZEK_TOUCHPAD_ABS: &str = "6f3800001000003";

#[test]
fn the_bitmap_decoder_reads_proc_the_way_the_kernel_prints_it() {
    let caps = from_proc("5", ZEK_TOUCHPAD_KEY, "", ZEK_TOUCHPAD_ABS);
    assert_eq!(
        caps.props.iter().copied().collect::<Vec<_>>(),
        vec![0, code::INPUT_PROP_BUTTONPAD]
    );
    for key in [code::BTN_LEFT, code::BTN_TOOL_FINGER, code::BTN_TOUCH] {
        assert!(
            caps.keys.contains(&key),
            "{key:#x} missing from {:?}",
            caps.keys
        );
    }
    assert_eq!(caps.keys.len(), 7, "{:?}", caps.keys);
    for axis in [
        code::ABS_MT_SLOT,
        code::ABS_MT_POSITION_X,
        code::ABS_MT_POSITION_Y,
        code::ABS_MT_TOOL_TYPE,
        code::ABS_MT_TRACKING_ID,
    ] {
        assert!(caps.abs.contains(&axis), "{axis:#x} missing");
    }
}

#[test]
fn zeks_touchpad_is_a_clickpad_touchpad_and_a_pointer() {
    let caps = from_proc("5", ZEK_TOUCHPAD_KEY, "", ZEK_TOUCHPAD_ABS);
    assert_eq!(
        caps.classify(),
        Class {
            keyboard: false,
            pointer: true,
            touchpad: true
        }
    );
    assert!(caps.is_clickpad());
}

#[test]
fn zeks_mouse_node_stays_a_plain_mouse() {
    let caps = from_proc("0", "30000 0 0 0 0", "3", "");
    assert_eq!(
        caps.classify(),
        Class {
            keyboard: false,
            pointer: true,
            touchpad: false
        }
    );
    assert!(!caps.is_clickpad());
}

#[test]
fn zeks_touchscreen_is_not_a_touchpad_and_not_a_pointer() {
    let caps = from_proc("2", "400 0 0 0 0 0", "", "260800000000003");
    assert_eq!(
        caps.classify(),
        Class {
            keyboard: false,
            pointer: false,
            touchpad: false
        }
    );
}

#[test]
fn zeks_stylus_is_not_a_touchpad_and_not_a_pointer() {
    let caps = from_proc("2", "1c03 0 0 0 0 0", "", "1000f000003");
    assert_eq!(
        caps.classify(),
        Class {
            keyboard: false,
            pointer: false,
            touchpad: false
        }
    );
}

#[test]
fn zeks_keyboard_stays_a_keyboard() {
    let key = "1c0000 0 0 0 0 1500f02000403 ff803078f870d001 febfffdffbcfffff fffffffffffffffe";
    let caps = from_proc("0", key, "", "");
    assert_eq!(
        caps.classify(),
        Class {
            keyboard: true,
            pointer: false,
            touchpad: false
        }
    );
}

#[test]
fn each_required_part_of_a_touchpad_is_required() {
    let full = || from_proc("5", ZEK_TOUCHPAD_KEY, "", ZEK_TOUCHPAD_ABS);
    assert!(full().classify().touchpad, "the control must pass");
    for key in [code::BTN_TOOL_FINGER, code::BTN_TOUCH] {
        let mut caps = full();
        caps.keys.remove(&key);
        assert!(!caps.classify().touchpad, "without key {key:#x}");
    }
    for axis in [
        code::ABS_MT_SLOT,
        code::ABS_MT_POSITION_X,
        code::ABS_MT_POSITION_Y,
        code::ABS_MT_TRACKING_ID,
    ] {
        let mut caps = full();
        caps.abs.remove(&axis);
        assert!(!caps.classify().touchpad, "without axis {axis:#x}");
    }
}

#[test]
fn each_refusing_part_of_a_touchpad_refuses() {
    let full = || from_proc("5", ZEK_TOUCHPAD_KEY, "", ZEK_TOUCHPAD_ABS);
    let mut direct = full();
    direct.props.insert(code::INPUT_PROP_DIRECT);
    assert!(!direct.classify().touchpad, "a direct device is a screen");
    let mut relative = full();
    relative.rel.insert(code::REL_X);
    assert!(!relative.classify().touchpad, "REL_X is a mouse");
    let mut pen = full();
    pen.keys.insert(code::BTN_TOOL_PEN);
    assert!(!pen.classify().touchpad, "a pen is a tablet");
}

#[test]
fn a_keyboard_key_makes_a_keyboard_and_a_left_button_alone_stays_a_legacy_pointer() {
    assert!(from_proc("0", "40000000", "", "").classify().keyboard);
    let button_only = from_proc("0", "10000 0 0 0 0", "", "").classify();
    assert!(button_only.pointer && !button_only.touchpad);
    let rel_only = from_proc("0", "", "1", "").classify();
    assert!(rel_only.pointer && !rel_only.keyboard);
    let empty = from_proc("0", "", "", "").classify();
    assert!(!empty.pointer && !empty.keyboard && !empty.touchpad);
}

#[test]
fn a_node_with_only_seat_keys_is_a_keyboard() {
    let mut video_bus = from_proc("0", "", "", "");
    video_bus
        .keys
        .extend([code::KEY_BRIGHTNESSDOWN, code::KEY_BRIGHTNESSUP]);
    assert!(
        video_bus.classify().keyboard,
        "the ACPI Video Bus carries brightness keys"
    );
    let mut power_button = from_proc("0", "", "", "");
    power_button.keys.insert(116);
    assert!(
        !power_button.classify().keyboard,
        "logind owns the power button"
    );
}
