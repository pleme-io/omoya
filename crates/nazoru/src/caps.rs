use std::collections::BTreeSet;

use crate::code;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub props: BTreeSet<u16>,
    pub keys: BTreeSet<u16>,
    pub rel: BTreeSet<u16>,
    pub abs: BTreeSet<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Class {
    pub keyboard: bool,
    pub pointer: bool,
    pub touchpad: bool,
}

impl Capabilities {
    #[must_use]
    pub fn classify(&self) -> Class {
        Class {
            keyboard: self.keys.contains(&code::KEY_A),
            pointer: self.keys.contains(&code::BTN_LEFT) || self.rel.contains(&code::REL_X),
            touchpad: self.is_touchpad(),
        }
    }

    #[must_use]
    pub fn is_clickpad(&self) -> bool {
        self.props.contains(&code::INPUT_PROP_BUTTONPAD)
    }

    fn is_touchpad(&self) -> bool {
        !self.props.contains(&code::INPUT_PROP_DIRECT)
            && !self.rel.contains(&code::REL_X)
            && !self.keys.contains(&code::BTN_TOOL_PEN)
            && self.keys.contains(&code::BTN_TOOL_FINGER)
            && self.keys.contains(&code::BTN_TOUCH)
            && self.abs.contains(&code::ABS_MT_SLOT)
            && self.abs.contains(&code::ABS_MT_POSITION_X)
            && self.abs.contains(&code::ABS_MT_POSITION_Y)
            && self.abs.contains(&code::ABS_MT_TRACKING_ID)
    }
}
