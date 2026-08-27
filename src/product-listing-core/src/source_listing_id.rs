use uuid::Uuid;

domain_primitives::slug_id_newtype!(SourceListingId, 0);

impl SourceListingId {
    pub fn new() -> Self {
        Self::from(Uuid::new_v4().to_string())
    }
}

impl Default for SourceListingId {
    fn default() -> Self {
        Self::new()
    }
}
