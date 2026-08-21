use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct UserSearchFilterId(Uuid);

impl Default for UserSearchFilterId {
    fn default() -> Self {
        Self::new()
    }
}

impl UserSearchFilterId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for UserSearchFilterId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<Uuid> for UserSearchFilterId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<UserSearchFilterId> for String {
    fn from(value: UserSearchFilterId) -> Self {
        value.0.to_string()
    }
}

impl TryFrom<String> for UserSearchFilterId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl TryFrom<&str> for UserSearchFilterId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

impl TryFrom<&String> for UserSearchFilterId {
    type Error = uuid::Error;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}
