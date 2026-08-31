use std::fmt::{Display, Formatter};

use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ListingSourceId(Uuid);

impl ListingSourceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ListingSourceId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ListingSourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for ListingSourceId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<ListingSourceId> for Uuid {
    fn from(value: ListingSourceId) -> Self {
        value.0
    }
}

impl TryFrom<String> for ListingSourceId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl TryFrom<&str> for ListingSourceId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

impl From<ListingSourceId> for String {
    fn from(value: ListingSourceId) -> Self {
        value.0.to_string()
    }
}
