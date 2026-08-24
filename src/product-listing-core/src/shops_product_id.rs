use uuid::Uuid;

domain_primitives::slug_id_newtype!(ShopsProductId, 0);

impl ShopsProductId {
    pub fn new() -> Self {
        Self::from(Uuid::new_v4().to_string())
    }
}

impl Default for ShopsProductId {
    fn default() -> Self {
        Self::new()
    }
}
