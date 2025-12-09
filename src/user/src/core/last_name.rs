use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    ops::Deref,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastName(
    #[cfg_attr(
        feature = "test-data",
        dummy(faker = "fake::faker::name::en::LastName()")
    )]
    String,
);

impl Display for LastName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for LastName {
    fn from(s: &str) -> Self {
        if s.len() > 64 {
            match s.split_at_checked(64) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            LastName(s.into())
        }
    }
}

impl From<String> for LastName {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<LastName> for String {
    fn from(t: LastName) -> Self {
        t.0
    }
}

impl Deref for LastName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for LastName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
