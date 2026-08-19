use crate::shop_slug_id::ShopSlugId;
use std::{fmt::Display, ops::Deref};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct SellerSlugId(ShopSlugId);

impl SellerSlugId {
    pub fn raw<S: AsRef<str>>(value: S) -> Result<Self, crate::shop_slug_id::InvalidShopSlugId> {
        ShopSlugId::raw(value).map(Self)
    }
}

impl Display for SellerSlugId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<SellerSlugId> for String {
    fn from(value: SellerSlugId) -> Self {
        value.0.into()
    }
}

impl From<String> for SellerSlugId {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<&String> for SellerSlugId {
    fn from(value: &String) -> Self {
        Self(value.into())
    }
}

impl From<&str> for SellerSlugId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl From<ShopSlugId> for SellerSlugId {
    fn from(value: ShopSlugId) -> Self {
        Self(value)
    }
}

impl From<SellerSlugId> for ShopSlugId {
    fn from(value: SellerSlugId) -> Self {
        value.0
    }
}

impl AsRef<str> for SellerSlugId {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Deref for SellerSlugId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
