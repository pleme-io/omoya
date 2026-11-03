pub const FALLBACK_WIDTH_MM: f64 = 100.0;
pub const MAX_SLOTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbsAxis {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub resolution: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    minimum: f64,
    extent: f64,
    units_per_mm: f64,
}

impl Span {
    fn mm(self, value: i32) -> f64 {
        f64::from(value) / self.units_per_mm
    }

    fn fraction(self, value: i32) -> f64 {
        (f64::from(value) - self.minimum) / self.extent
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    x: Span,
    y: Span,
    slots: usize,
    first_slot: Option<usize>,
}

impl Surface {
    #[must_use]
    pub fn new(x: AbsAxis, y: AbsAxis, slot: AbsAxis) -> Option<Self> {
        if x.maximum <= x.minimum || y.maximum <= y.minimum || slot.maximum < slot.minimum {
            return None;
        }
        let extent_x = f64::from(x.maximum) - f64::from(x.minimum);
        let extent_y = f64::from(y.maximum) - f64::from(y.minimum);
        let fallback = extent_x / FALLBACK_WIDTH_MM;
        let (units_x, units_y) = match (x.resolution > 0, y.resolution > 0) {
            (true, true) => (f64::from(x.resolution), f64::from(y.resolution)),
            (true, false) => (f64::from(x.resolution), f64::from(x.resolution)),
            (false, true) => (f64::from(y.resolution), f64::from(y.resolution)),
            (false, false) => (fallback, fallback),
        };
        let slots = usize::try_from(i64::from(slot.maximum) - i64::from(slot.minimum) + 1)
            .map_or(MAX_SLOTS, |count| count.min(MAX_SLOTS));
        Some(Self {
            x: Span {
                minimum: f64::from(x.minimum),
                extent: extent_x,
                units_per_mm: units_x,
            },
            y: Span {
                minimum: f64::from(y.minimum),
                extent: extent_y,
                units_per_mm: units_y,
            },
            slots,
            first_slot: usize::try_from(slot.value).ok(),
        })
    }

    #[must_use]
    pub fn width_mm(&self) -> f64 {
        self.x.extent / self.x.units_per_mm
    }

    #[must_use]
    pub fn height_mm(&self) -> f64 {
        self.y.extent / self.y.units_per_mm
    }

    #[must_use]
    pub fn slots(&self) -> usize {
        self.slots
    }

    pub(crate) fn first_slot(&self) -> Option<usize> {
        self.first_slot
    }

    pub(crate) fn mm(&self, x: i32, y: i32) -> (f64, f64) {
        (self.x.mm(x), self.y.mm(y))
    }

    pub(crate) fn fraction(&self, x: i32, y: i32) -> (f64, f64) {
        (self.x.fraction(x), self.y.fraction(y))
    }
}
