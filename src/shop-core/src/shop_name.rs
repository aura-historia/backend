use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    ops::Deref,
};
use strum::EnumCount;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShopName(
    #[cfg_attr(
        feature = "test-data",
        dummy(faker = "fake::faker::company::en::CompanyName()")
    )]
    String,
);

impl Display for ShopName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<&str> for ShopName {
    fn from(value: &str) -> Self {
        if value.len() > 255 {
            match value.split_at_checked(255) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(value.into()),
            }
        } else {
            Self(value.into())
        }
    }
}

impl From<String> for ShopName {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<ShopName> for String {
    fn from(value: ShopName) -> Self {
        value.0
    }
}

impl Deref for ShopName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for ShopName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl EnumCount for ShopName {
    const COUNT: usize = usize::MAX;
}
