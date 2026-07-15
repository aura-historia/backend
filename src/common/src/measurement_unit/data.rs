use crate::measurement_unit::domain::MeasurementUnit;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MeasurementUnitData {
    #[default]
    Metric,
    Imperial,
}

impl From<MeasurementUnit> for MeasurementUnitData {
    fn from(domain: MeasurementUnit) -> Self {
        match domain {
            MeasurementUnit::Metric => MeasurementUnitData::Metric,
            MeasurementUnit::Imperial => MeasurementUnitData::Imperial,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MeasurementUnitData;
    use rstest::rstest;

    #[rstest]
    #[case(MeasurementUnitData::Metric, "\"METRIC\"")]
    #[case(MeasurementUnitData::Imperial, "\"IMPERIAL\"")]
    #[trace]
    fn should_serialize_measurement_unit(
        #[case] measurement_unit: MeasurementUnitData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&measurement_unit).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"METRIC\"", MeasurementUnitData::Metric)]
    #[case("\"IMPERIAL\"", MeasurementUnitData::Imperial)]
    #[trace]
    fn should_deserialize_measurement_unit(
        #[case] measurement_unit: &str,
        #[case] expected: MeasurementUnitData,
    ) {
        let actual = serde_json::from_str::<MeasurementUnitData>(measurement_unit).unwrap();
        assert_eq!(actual, expected);
    }
}
