use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct ShopId(Uuid);

impl Default for ShopId {
    fn default() -> Self {
        Self::new()
    }
}

impl ShopId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for ShopId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<Uuid> for ShopId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<ShopId> for Uuid {
    fn from(value: ShopId) -> Self {
        value.0
    }
}

impl TryFrom<String> for ShopId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl TryFrom<&str> for ShopId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

impl TryFrom<&String> for ShopId {
    type Error = uuid::Error;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<ShopId> for String {
    fn from(value: ShopId) -> Self {
        value.0.to_string()
    }
}
