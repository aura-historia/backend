use uuid::Uuid;

domain_primitives::slug_id_newtype!(ShopListingId, 0);

impl ShopListingId {
    pub fn new() -> Self {
        Self::from(Uuid::new_v4().to_string())
    }
}

impl Default for ShopListingId {
    fn default() -> Self {
        Self::new()
    }
}
