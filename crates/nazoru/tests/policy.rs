mod rig;

use std::time::Duration;

use nazoru::{
    AbsAxis, Acceleration, BASE_PX_PER_MM, BoundedKnee, BoundedMaxGain, BoundedScale, BoundedSlope,
    BoundedSpeed, FALLBACK_WIDTH_MM, MAX_SLOTS, MIN_DT, PointerTransfer, ScrollDirection, Surface,
    TouchpadConfig,
};
use rig::{frame, motions};

const MS_8: Duration = Duration::from_millis(8);

fn axis(minimum: i32, maximum: i32, resolution: i32) -> AbsAxis {
    AbsAxis {
        value: 0,
        minimum,
        maximum,
        resolution,
    }
}

fn adaptive() -> PointerTransfer {
    PointerTransfer::Adaptive {
        scale: BoundedScale::new(10.0),
        knee: BoundedKnee::new(50.0),
        slope: BoundedSlope::new(0.01),
        max_gain: BoundedMaxGain::new(2.5),
    }
}

#[test]
fn the_default_transfer_is_the_identity_bit_for_bit() {
    let identity = PointerTransfer::default();
    for raw in [
        (0.0, 0.0),
        (1.0, -1.0),
        (0.1, 0.2),
        (1e-12, 1e12),
        (-3.5, 7.25),
    ] {
        for dt in [Duration::ZERO, MS_8, Duration::from_secs(60)] {
            assert_eq!(identity.apply(raw, dt), raw);
        }
    }
}

#[test]
fn flat_scales_and_ignores_speed() {
    let flat = PointerTransfer::Flat {
        scale: BoundedScale::new(10.0),
    };
    assert_eq!(flat.apply((1.0, 2.0), MS_8), (10.0, 20.0));
    assert_eq!(flat.apply((1.0, 2.0), Duration::from_secs(1)), (10.0, 20.0));
}

#[test]
fn adaptive_is_flat_at_or_under_the_knee() {
    let at_knee = adaptive().apply((0.4, 0.0), MS_8);
    assert!((at_knee.0 - 4.0).abs() < 1e-9, "{at_knee:?}");
    let under = adaptive().apply((0.1, 0.0), MS_8);
    assert!((under.0 - 1.0).abs() < 1e-9, "{under:?}");
}

#[test]
fn adaptive_rises_linearly_past_the_knee_and_stops_at_the_ceiling() {
    let mid = adaptive().apply((0.8, 0.0), MS_8);
    assert!(
        (mid.0 - 8.0 * 1.5).abs() < 1e-9,
        "100 mm/s is gain 1.5: {mid:?}"
    );
    let top = adaptive().apply((1.6, 0.0), MS_8);
    assert!(
        (top.0 - 16.0 * 2.5).abs() < 1e-9,
        "200 mm/s reaches 2.5: {top:?}"
    );
    let past = adaptive().apply((8.0, 0.0), MS_8);
    assert!(
        (past.0 - 80.0 * 2.5).abs() < 1e-9,
        "never past the ceiling: {past:?}"
    );
}

#[test]
fn adaptive_keeps_direction_exactly() {
    let (x, y) = adaptive().apply((3.0, -4.0), MS_8);
    assert!((x / y + 0.75).abs() < 1e-12, "{x} {y}");
    assert!(x > 0.0 && y < 0.0);
}

#[test]
fn adaptive_is_monotonic_in_speed() {
    let mut last = 0.0;
    for step in 1..200 {
        let raw = f64::from(step) * 0.05;
        let (x, _) = adaptive().apply((raw, 0.0), MS_8);
        assert!(x >= last, "{x} after {last} at {raw}");
        last = x;
    }
}

#[test]
fn a_zero_dt_means_unknown_speed_and_a_tiny_dt_is_floored() {
    assert_eq!(adaptive().apply((1.0, 0.0), Duration::ZERO), (10.0, 0.0));
    let tiny = adaptive().apply((0.2, 0.0), Duration::from_micros(1));
    assert!(
        (tiny.0 - 2.0).abs() < 1e-9,
        "floored at {MIN_DT:?} this is 50 mm/s, the knee: {tiny:?}"
    );
    let floor = adaptive().apply((0.2, 0.0), MIN_DT);
    assert_eq!(tiny, floor);
}

#[test]
fn a_zero_delta_stays_zero() {
    assert_eq!(adaptive().apply((0.0, 0.0), MS_8), (0.0, 0.0));
}

#[test]
fn the_measured_zek_surface_is_118_by_81_mm_with_five_slots() {
    let surface = rig::zek_surface();
    assert!((surface.width_mm() - 118.0).abs() < 1e-9);
    assert!((surface.height_mm() - 81.0).abs() < 1e-9);
    assert_eq!(surface.slots(), 5);
}

#[test]
fn a_device_reporting_no_resolution_still_gets_a_usable_scale() {
    let surface = Surface::new(axis(0, 1000, 0), axis(0, 500, 0), axis(0, 4, 0)).unwrap();
    assert!((surface.width_mm() - FALLBACK_WIDTH_MM).abs() < 1e-9);
    assert!((surface.height_mm() - FALLBACK_WIDTH_MM / 2.0).abs() < 1e-9);
}

#[test]
fn one_axis_missing_resolution_borrows_the_other() {
    let x_missing = Surface::new(axis(0, 1000, 0), axis(0, 500, 10), axis(0, 4, 0)).unwrap();
    assert!((x_missing.width_mm() - 100.0).abs() < 1e-9);
    assert!((x_missing.height_mm() - 50.0).abs() < 1e-9);
    let y_missing = Surface::new(axis(0, 1000, 20), axis(0, 500, -1), axis(0, 4, 0)).unwrap();
    assert!((y_missing.width_mm() - 50.0).abs() < 1e-9);
    assert!((y_missing.height_mm() - 25.0).abs() < 1e-9);
}

#[test]
fn an_axis_with_no_extent_refuses_this_device_alone() {
    assert!(Surface::new(axis(0, 0, 16), axis(0, 100, 16), axis(0, 4, 0)).is_none());
    assert!(Surface::new(axis(0, 100, 16), axis(100, 100, 16), axis(0, 4, 0)).is_none());
    assert!(Surface::new(axis(10, 5, 16), axis(0, 100, 16), axis(0, 4, 0)).is_none());
    assert!(Surface::new(axis(0, 100, 16), axis(0, 100, 16), axis(4, 3, 0)).is_none());
    assert!(Surface::new(axis(0, 100, 16), axis(0, 100, 16), axis(0, 0, 0)).is_some());
}

#[test]
fn an_absurd_slot_count_is_capped() {
    let huge = Surface::new(axis(0, 100, 16), axis(0, 100, 16), axis(0, i32::MAX, 0)).unwrap();
    assert_eq!(huge.slots(), MAX_SLOTS);
    let wide = Surface::new(
        axis(0, 100, 16),
        axis(0, 100, 16),
        axis(i32::MIN, i32::MAX, 0),
    );
    assert_eq!(wide.unwrap().slots(), MAX_SLOTS);
}

#[test]
fn axes_offset_from_zero_measure_their_fractions_from_the_minimum() {
    let surface = Surface::new(axis(100, 1100, 10), axis(200, 1200, 10), axis(0, 4, 0)).unwrap();
    let mut pad = nazoru::Touchpad::new(TouchpadConfig::default(), surface, true);
    frame(
        &mut pad,
        0,
        &[rig::Step::Down(0, 1, 1000, 1100), rig::Step::Press],
    );
    assert_eq!(
        rig::buttons(&frame(&mut pad, 8, &[rig::Step::Release])),
        vec![(u32::from(nazoru::code::BTN_RIGHT), false, 8_000)]
    );
    let mut pad = nazoru::Touchpad::new(TouchpadConfig::default(), surface, true);
    frame(
        &mut pad,
        0,
        &[rig::Step::Down(0, 1, 500, 1100), rig::Step::Press],
    );
    assert_eq!(
        rig::buttons(&frame(&mut pad, 8, &[rig::Step::Release])),
        vec![(u32::from(nazoru::code::BTN_LEFT), false, 8_000)]
    );
}

#[test]
fn the_default_config_is_what_the_brief_asked_for() {
    let config = TouchpadConfig::default();
    assert!(config.enabled);
    assert!(config.tap_to_click);
    assert_eq!(config.scroll_direction, ScrollDirection::Natural);
    assert_eq!(config.acceleration, Acceleration::Adaptive);
    assert!((config.speed.get() - 1.0).abs() < f64::EPSILON);
    assert_eq!(config.tap_max_ms.get(), 180);
    assert!((config.tap_max_travel_mm.get() - 2.0).abs() < f64::EPSILON);
    assert!((config.scroll_speed.get() - 1.0).abs() < f64::EPSILON);
    assert!((config.right_click_width_pct.get() - 50.0).abs() < f64::EPSILON);
    assert!((config.right_click_height_pct.get() - 20.0).abs() < f64::EPSILON);
}

#[test]
fn an_empty_block_is_the_default_and_a_partial_block_keeps_the_rest() {
    let empty: TouchpadConfig = serde_yaml::from_str("{}").unwrap();
    assert_eq!(empty, TouchpadConfig::default());
    let partial: TouchpadConfig = serde_yaml::from_str("speed: 2.0\n").unwrap();
    assert!((partial.speed.get() - 2.0).abs() < f64::EPSILON);
    assert!(partial.tap_to_click);
    assert_eq!(partial.scroll_direction, ScrollDirection::Natural);
}

#[test]
fn every_key_parses_under_the_name_the_report_promises() {
    let yaml = "\
enabled: false
speed: 1.5
acceleration: flat
accel_knee_mm_s: 40
accel_slope: 0.02
accel_max_gain: 3.0
tap_to_click: false
tap_max_ms: 120
tap_max_travel_mm: 1.5
scroll_direction: traditional
scroll_speed: 0.5
right_click_width_pct: 40
right_click_height_pct: 15
";
    let config: TouchpadConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(!config.enabled);
    assert!((config.speed.get() - 1.5).abs() < f64::EPSILON);
    assert_eq!(config.acceleration, Acceleration::Flat);
    assert!((config.accel_knee_mm_s.get() - 40.0).abs() < f64::EPSILON);
    assert!((config.accel_slope.get() - 0.02).abs() < f64::EPSILON);
    assert!((config.accel_max_gain.get() - 3.0).abs() < f64::EPSILON);
    assert!(!config.tap_to_click);
    assert_eq!(config.tap_max_ms.get(), 120);
    assert!((config.tap_max_travel_mm.get() - 1.5).abs() < f64::EPSILON);
    assert_eq!(config.scroll_direction, ScrollDirection::Traditional);
    assert!((config.scroll_speed.get() - 0.5).abs() < f64::EPSILON);
    assert!((config.right_click_width_pct.get() - 40.0).abs() < f64::EPSILON);
    assert!((config.right_click_height_pct.get() - 15.0).abs() < f64::EPSILON);
    let back: TouchpadConfig =
        serde_yaml::from_str(&serde_yaml::to_string(&config).unwrap()).unwrap();
    assert_eq!(config, back);
}

#[test]
fn out_of_band_numbers_clamp_rather_than_refuse_the_seat() {
    let wild: TouchpadConfig = serde_yaml::from_str(
        "speed: 999\naccel_knee_mm_s: -5\naccel_slope: 9\naccel_max_gain: 0\ntap_max_ms: 0\n\
         tap_max_travel_mm: 99\nscroll_speed: 0\nright_click_width_pct: 0\nright_click_height_pct: 99\n",
    )
    .unwrap();
    assert!((wild.speed.get() - 5.0).abs() < f64::EPSILON);
    assert!(wild.accel_knee_mm_s.get().abs() < f64::EPSILON);
    assert!((wild.accel_slope.get() - 0.1).abs() < f64::EPSILON);
    assert!((wild.accel_max_gain.get() - 1.0).abs() < f64::EPSILON);
    assert_eq!(wild.tap_max_ms.get(), 50);
    assert!((wild.tap_max_travel_mm.get() - 10.0).abs() < f64::EPSILON);
    assert!((wild.scroll_speed.get() - 0.1).abs() < f64::EPSILON);
    assert!((wild.right_click_width_pct.get() - 10.0).abs() < f64::EPSILON);
    assert!((wild.right_click_height_pct.get() - 50.0).abs() < f64::EPSILON);
}

#[test]
fn a_typod_key_or_an_unknown_word_is_refused() {
    assert!(serde_yaml::from_str::<TouchpadConfig>("sped: 1\n").is_err());
    assert!(serde_yaml::from_str::<TouchpadConfig>("acceleration: wild\n").is_err());
    assert!(serde_yaml::from_str::<TouchpadConfig>("scroll_direction: up\n").is_err());
}

#[test]
fn the_speed_setting_reaches_the_transfer_and_acceleration_picks_its_shape() {
    let flat = TouchpadConfig {
        acceleration: Acceleration::Flat,
        speed: BoundedSpeed::new(2.0),
        ..TouchpadConfig::default()
    };
    assert_eq!(
        flat.transfer(),
        PointerTransfer::Flat {
            scale: BoundedScale::new(2.0 * BASE_PX_PER_MM)
        }
    );
    let curve = TouchpadConfig {
        speed: BoundedSpeed::new(2.0),
        accel_knee_mm_s: BoundedKnee::new(30.0),
        accel_slope: BoundedSlope::new(0.02),
        accel_max_gain: BoundedMaxGain::new(1.5),
        ..TouchpadConfig::default()
    };
    assert_eq!(
        curve.transfer(),
        PointerTransfer::Adaptive {
            scale: BoundedScale::new(2.0 * BASE_PX_PER_MM),
            knee: BoundedKnee::new(30.0),
            slope: BoundedSlope::new(0.02),
            max_gain: BoundedMaxGain::new(1.5),
        }
    );
}

#[test]
fn the_acceleration_setting_reaches_the_machine() {
    let mut pad = rig::zek_pad();
    frame(&mut pad, 0, &[rig::Step::Down(0, 1, 500, 300)]);
    let accelerated = frame(&mut pad, 8, &[rig::Step::Move(0, 564, 300)]);
    let config = TouchpadConfig {
        acceleration: Acceleration::Flat,
        ..TouchpadConfig::default()
    };
    let mut pad = rig::zek_pad_with(config);
    frame(&mut pad, 0, &[rig::Step::Down(0, 1, 500, 300)]);
    let flat = frame(&mut pad, 8, &[rig::Step::Move(0, 564, 300)]);
    assert!(motions(&accelerated)[0].0 > motions(&flat)[0].0);
}
