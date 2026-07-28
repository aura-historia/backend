common::uuid_v4_newtype!(FxRateId);

impl From<FxRateId> for uuid::Uuid {
    fn from(id: FxRateId) -> Self {
        id.0
    }
}
