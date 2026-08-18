crate::uuid_v7_newtype!(PartnerShopApplicationId);

impl From<PartnerShopApplicationId> for uuid::Uuid {
    fn from(id: PartnerShopApplicationId) -> Self {
        id.0
    }
}
