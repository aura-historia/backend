crate::uuid_v4_newtype!(FxRateId);

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
}
