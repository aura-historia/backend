use super::*;
use user_core::measurement_unit::MeasurementUnit;

pub mod option {
    use super::*;

    pub fn serialize<S>(value: &Option<MeasurementUnit>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_option_code(value, serializer, MeasurementUnit::as_str)
    }
}

pub mod patch {
    use super::*;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<MeasurementUnit>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_patch_code(deserializer, MeasurementUnit::from_code)
    }
}
