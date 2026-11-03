#![forbid(unsafe_code)]

mod caps;
pub mod code;
mod config;
mod machine;
mod surface;
mod transfer;

pub use caps::{Capabilities, Class};
pub use config::{
    Acceleration, BoundedAreaHeight, BoundedAreaWidth, BoundedKnee, BoundedMaxGain, BoundedScale,
    BoundedScrollSpeed, BoundedSlope, BoundedSpeed, BoundedTapMs, BoundedTapTravel,
    ScrollDirection, TouchpadConfig,
};
pub use machine::{
    AXIS_VALUE_PER_DETENT, Action, MAX_JUMP_MM, MM_PER_DETENT, RawEvent, Touchpad, V120_PER_DETENT,
    axis_value, owned_by_pad,
};
pub use surface::{AbsAxis, FALLBACK_WIDTH_MM, MAX_SLOTS, Surface};
pub use transfer::{BASE_PX_PER_MM, MIN_DT, PointerTransfer};
