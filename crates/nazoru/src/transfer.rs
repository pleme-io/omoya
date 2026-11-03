use std::time::Duration;

use crate::config::{BoundedKnee, BoundedMaxGain, BoundedScale, BoundedSlope};

pub const BASE_PX_PER_MM: f64 = 10.0;
pub const MIN_DT: Duration = Duration::from_millis(4);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerTransfer {
    Flat {
        scale: BoundedScale,
    },
    Adaptive {
        scale: BoundedScale,
        knee: BoundedKnee,
        slope: BoundedSlope,
        max_gain: BoundedMaxGain,
    },
}

impl Default for PointerTransfer {
    fn default() -> Self {
        Self::Flat {
            scale: BoundedScale::new(1.0),
        }
    }
}

impl PointerTransfer {
    #[must_use]
    pub fn apply(self, raw: (f64, f64), dt: Duration) -> (f64, f64) {
        match self {
            Self::Flat { scale } => (raw.0 * scale.get(), raw.1 * scale.get()),
            Self::Adaptive {
                scale,
                knee,
                slope,
                max_gain,
            } => {
                let gain = if dt.is_zero() {
                    1.0
                } else {
                    let speed = raw.0.hypot(raw.1) / dt.max(MIN_DT).as_secs_f64();
                    let excess = (speed - knee.get()).max(0.0);
                    (1.0 + slope.get() * excess).min(max_gain.get())
                };
                (raw.0 * scale.get() * gain, raw.1 * scale.get() * gain)
            }
        }
    }
}
