use crate::measurement_unit::domain::MeasurementUnit;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MeasurementUnitRecord {
    Metric,
    Imperial,
}

impl From<MeasurementUnit> for MeasurementUnitRecord {
    fn from(domain: MeasurementUnit) -> Self {
        match domain {
            MeasurementUnit::Metric => MeasurementUnitRecord::Metric,
            MeasurementUnit::Imperial => MeasurementUnitRecord::Imperial,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MeasurementUnitRecord;
    use rstest::rstest;

    #[rstest]
    #[case(MeasurementUnitRecord::Metric, "\"METRIC\"")]
    #[case(MeasurementUnitRecord::Imperial, "\"IMPERIAL\"")]
    #[trace]
    fn should_serialize_measurement_unit_record(
        #[case] measurement_unit: MeasurementUnitRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&measurement_unit).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"METRIC\"", MeasurementUnitRecord::Metric)]
    #[case("\"IMPERIAL\"", MeasurementUnitRecord::Imperial)]
    #[trace]
    fn should_deserialize_measurement_unit_record(
        #[case] measurement_unit: &str,
        #[case] expected: MeasurementUnitRecord,
    ) {
        let actual = serde_json::from_str::<MeasurementUnitRecord>(measurement_unit).unwrap();
        assert_eq!(actual, expected);
    }
}
