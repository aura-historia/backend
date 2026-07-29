common::uuid_v4_newtype!(FxRateId);

impl From<FxRateId> for uuid::Uuid {
    fn from(id: FxRateId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_to_uuid() {
        let uuid = uuid::Uuid::new_v4();
        let id = FxRateId::from(uuid);

        assert_eq!(uuid, uuid::Uuid::from(id));
    }

    #[test]
    fn should_parse_from_string() {
        let uuid = uuid::Uuid::new_v4();
        let value = uuid.to_string();

        let result = FxRateId::try_from(value);

        assert!(matches!(result, Ok(id) if uuid::Uuid::from(id) == uuid));
    }
}
