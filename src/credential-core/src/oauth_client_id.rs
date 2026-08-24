use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct OAuthClientId(Uuid);

impl Default for OAuthClientId {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuthClientId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for OAuthClientId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<Uuid> for OAuthClientId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl TryFrom<String> for OAuthClientId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl From<OAuthClientId> for String {
    fn from(value: OAuthClientId) -> Self {
        value.0.to_string()
    }
}

impl TryFrom<&str> for OAuthClientId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

impl TryFrom<&String> for OAuthClientId {
    type Error = uuid::Error;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}
