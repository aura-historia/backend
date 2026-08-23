#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, strum_macros::EnumIter)]
pub enum MeasurementUnit {
    #[default]
    Metric,
    Imperial,
}

impl MeasurementUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "METRIC",
            Self::Imperial => "IMPERIAL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MeasurementUnit;
    use strum::IntoEnumIterator;

    #[test]
    fn should_render_canonical_measurement_unit_identifiers() {
        assert_eq!("METRIC", MeasurementUnit::Metric.as_str());
        assert_eq!("IMPERIAL", MeasurementUnit::Imperial.as_str());
    }

    #[test]
    fn should_have_unique_measurement_unit_identifiers() {
        let identifiers = MeasurementUnit::iter()
            .map(MeasurementUnit::as_str)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(MeasurementUnit::iter().count(), identifiers.len());
    }
}
