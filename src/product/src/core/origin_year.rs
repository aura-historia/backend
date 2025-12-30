use common::year::{Year, YearRange};

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
