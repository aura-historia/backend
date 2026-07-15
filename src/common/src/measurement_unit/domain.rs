use crate::measurement_unit::{data::MeasurementUnitData, record::MeasurementUnitRecord};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
pub enum MeasurementUnit {
    #[default]
    Metric,
    Imperial,
}

impl From<MeasurementUnitData> for MeasurementUnit {
    fn from(data: MeasurementUnitData) -> Self {
        match data {
            MeasurementUnitData::Metric => MeasurementUnit::Metric,
            MeasurementUnitData::Imperial => MeasurementUnit::Imperial,
        }
    }
}

impl From<MeasurementUnitRecord> for MeasurementUnit {
    fn from(record: MeasurementUnitRecord) -> Self {
        match record {
            MeasurementUnitRecord::Metric => MeasurementUnit::Metric,
            MeasurementUnitRecord::Imperial => MeasurementUnit::Imperial,
        }
    }
}
