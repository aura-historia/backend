use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartnershipApplicationId(Uuid);

impl PartnershipApplicationId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}
impl Default for PartnershipApplicationId {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Display for PartnershipApplicationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl From<Uuid> for PartnershipApplicationId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}
impl From<PartnershipApplicationId> for Uuid {
    fn from(value: PartnershipApplicationId) -> Self {
        value.0
    }
}
