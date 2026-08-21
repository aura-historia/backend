domain_primitives::uuid_v4_newtype!(FxRateId);

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

        assert_eq!(uuid, uuid::Uuid::from(FxRateId::from(uuid)));
    }

    #[test]
    fn should_serialize_as_uuid_string() -> Result<(), serde_json::Error> {
        let id = FxRateId::from(uuid::uuid!("10000000-0000-0000-0000-000000000001"));

        assert_eq!(
            "\"10000000-0000-0000-0000-000000000001\"",
            serde_json::to_string(&id)?
        );
        Ok(())
    }
}
