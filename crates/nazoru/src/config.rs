use serde::{Deserialize, Serialize};

use crate::transfer::{BASE_PX_PER_MM, PointerTransfer};

macro_rules! bounded {
    ($alias:ident, $bounds:ident, $number:ty, $min:expr, $max:expr, $default:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct $bounds;

        impl ishou_tokens::Bounds<$number> for $bounds {
            fn min() -> $number {
                $min
            }
            fn max() -> $number {
                $max
            }
            fn default() -> $number {
                $default
            }
        }

        pub type $alias = ishou_tokens::Refined<$number, $bounds>;
    };
}

bounded!(BoundedScale, ScaleBounds, f64, 0.01, 100.0, 1.0);
bounded!(BoundedSpeed, SpeedBounds, f64, 0.1, 5.0, 1.0);
bounded!(BoundedKnee, KneeBounds, f64, 0.0, 500.0, 50.0);
bounded!(BoundedSlope, SlopeBounds, f64, 0.0, 0.1, 0.01);
bounded!(BoundedMaxGain, MaxGainBounds, f64, 1.0, 5.0, 2.5);
bounded!(BoundedTapMs, TapMsBounds, i32, 50, 500, 180);
bounded!(BoundedTapTravel, TapTravelBounds, f64, 0.5, 10.0, 2.0);
bounded!(BoundedScrollSpeed, ScrollSpeedBounds, f64, 0.1, 5.0, 1.0);
bounded!(BoundedAreaWidth, AreaWidthBounds, f64, 10.0, 100.0, 50.0);
bounded!(BoundedAreaHeight, AreaHeightBounds, f64, 5.0, 50.0, 20.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollDirection {
    #[default]
    Traditional,
    Natural,
}

impl ScrollDirection {
    #[must_use]
    pub const fn sign(self) -> f64 {
        match self {
            Self::Traditional => 1.0,
            Self::Natural => -1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Acceleration {
    Flat,
    #[default]
    Adaptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TouchpadConfig {
    pub enabled: bool,
    pub speed: BoundedSpeed,
    pub acceleration: Acceleration,
    pub accel_knee_mm_s: BoundedKnee,
    pub accel_slope: BoundedSlope,
    pub accel_max_gain: BoundedMaxGain,
    pub tap_to_click: bool,
    pub tap_max_ms: BoundedTapMs,
    pub tap_max_travel_mm: BoundedTapTravel,
    pub scroll_direction: ScrollDirection,
    pub scroll_speed: BoundedScrollSpeed,
    pub right_click_width_pct: BoundedAreaWidth,
    pub right_click_height_pct: BoundedAreaHeight,
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            speed: BoundedSpeed::default(),
            acceleration: Acceleration::default(),
            accel_knee_mm_s: BoundedKnee::default(),
            accel_slope: BoundedSlope::default(),
            accel_max_gain: BoundedMaxGain::default(),
            tap_to_click: true,
            tap_max_ms: BoundedTapMs::default(),
            tap_max_travel_mm: BoundedTapTravel::default(),
            scroll_direction: ScrollDirection::Natural,
            scroll_speed: BoundedScrollSpeed::default(),
            right_click_width_pct: BoundedAreaWidth::default(),
            right_click_height_pct: BoundedAreaHeight::default(),
        }
    }
}

impl TouchpadConfig {
    #[must_use]
    pub fn transfer(&self) -> PointerTransfer {
        let scale = BoundedScale::new(BASE_PX_PER_MM * self.speed.get());
        match self.acceleration {
            Acceleration::Flat => PointerTransfer::Flat { scale },
            Acceleration::Adaptive => PointerTransfer::Adaptive {
                scale,
                knee: self.accel_knee_mm_s,
                slope: self.accel_slope,
                max_gain: self.accel_max_gain,
            },
        }
    }
}
