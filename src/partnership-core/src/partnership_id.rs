use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartnershipId(Uuid);

impl PartnershipId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}
impl Default for PartnershipId {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Display for PartnershipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl From<Uuid> for PartnershipId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}
impl From<PartnershipId> for Uuid {
    fn from(value: PartnershipId) -> Self {
        value.0
    }
}
