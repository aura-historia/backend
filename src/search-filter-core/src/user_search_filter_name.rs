use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::ops::Deref;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct UserSearchFilterName(String);

impl From<&str> for UserSearchFilterName {
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

impl From<String> for UserSearchFilterName {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<UserSearchFilterName> for String {
    fn from(value: UserSearchFilterName) -> Self {
        value.0
    }
}

impl Display for UserSearchFilterName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Deref for UserSearchFilterName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for UserSearchFilterName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
