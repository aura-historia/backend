use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct PartnerShopApplicationId(Uuid);

impl Default for PartnerShopApplicationId {
    fn default() -> Self {
        Self::new()
    }
}

impl PartnerShopApplicationId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for PartnerShopApplicationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<Uuid> for PartnerShopApplicationId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<PartnerShopApplicationId> for String {
    fn from(value: PartnerShopApplicationId) -> Self {
        value.0.to_string()
    }
}

impl TryFrom<String> for PartnerShopApplicationId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl TryFrom<&str> for PartnerShopApplicationId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

impl TryFrom<&String> for PartnerShopApplicationId {
    type Error = uuid::Error;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}
