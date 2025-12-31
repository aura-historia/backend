use common::year::{Year, YearRange};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum OriginYear {
    ExactYear(Year),
    EstimatedRange(YearRange),
}

impl Default for OriginYear {
    fn default() -> Self {
        Self::EstimatedRange(Default::default())
    }
}

impl OriginYear {
    pub fn min(&self) -> Option<Year> {
        match self {
            OriginYear::ExactYear(_) => None,
            OriginYear::EstimatedRange(year_range) => year_range.min,
        }
    }

    pub fn max(&self) -> Option<Year> {
        match self {
            OriginYear::ExactYear(_) => None,
            OriginYear::EstimatedRange(year_range) => year_range.max,
        }
    }

    pub fn exact(&self) -> Option<Year> {
        match self {
            OriginYear::ExactYear(exact_year) => Some(*exact_year),
            OriginYear::EstimatedRange(_) => None,
        }
    }
}
