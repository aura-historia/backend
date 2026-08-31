use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PartyId(Uuid);

impl Default for PartyId {
    fn default() -> Self {
        Self::new()
    }
}

impl PartyId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for PartyId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<Uuid> for PartyId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<PartyId> for Uuid {
    fn from(value: PartyId) -> Self {
        value.0
    }
}
