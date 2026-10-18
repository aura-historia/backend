use crate::InvalidShippingValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Hours,
    Days,
    Weeks,
}
codes!(TimeUnit, Hours => "HOURS", Days => "DAYS", Weeks => "WEEKS");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeBasis {
    Calendar,
    Business,
}
codes!(TimeBasis, Calendar => "CALENDAR", Business => "BUSINESS");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeTrigger {
    Order,
    OrderConfirmation,
    Payment,
    Dispatch,
    ReadyForCollection,
}
codes!(TimeTrigger, Order => "ORDER", OrderConfirmation => "ORDER_CONFIRMATION", Payment => "PAYMENT", Dispatch => "DISPATCH", ReadyForCollection => "READY_FOR_COLLECTION");

/// A duration assertion, not a computed date. Missing basis/trigger remains absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    minimum: u32,
    maximum: u32,
    unit: TimeUnit,
    basis: Option<TimeBasis>,
    trigger: Option<TimeTrigger>,
}
impl TimeRange {
    pub fn new(
        minimum: u32,
        maximum: u32,
        unit: TimeUnit,
        basis: Option<TimeBasis>,
        trigger: Option<TimeTrigger>,
    ) -> Result<Self, InvalidShippingValue> {
        if minimum > maximum {
            return Err(InvalidShippingValue::Bounds);
        }
        Ok(Self {
            minimum,
            maximum,
            unit,
            basis,
            trigger,
        })
    }
    pub fn minimum(self) -> u32 {
        self.minimum
    }
    pub fn maximum(self) -> u32 {
        self.maximum
    }
    pub fn unit(self) -> TimeUnit {
        self.unit
    }
    pub fn basis(self) -> Option<TimeBasis> {
        self.basis
    }
    pub fn trigger(self) -> Option<TimeTrigger> {
        self.trigger
    }
}

/// These assertions are independent; assessment never adds incompatible time bases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LeadTimes {
    pub handling: Option<TimeRange>,
    pub transit: Option<TimeRange>,
    pub total: Option<TimeRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionArrangement {
    ByAppointment,
    ContactToArrange,
    Window(TimeRange),
}
impl CollectionArrangement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ByAppointment => "BY_APPOINTMENT",
            Self::ContactToArrange => "CONTACT_TO_ARRANGE",
            Self::Window(_) => "WINDOW",
        }
    }
}
