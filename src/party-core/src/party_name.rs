use std::{fmt::Display, ops::Deref};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartyName(String);

impl From<&str> for PartyName {
    fn from(value: &str) -> Self {
        Self(value.chars().take(255).collect())
    }
}

impl From<String> for PartyName {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl Display for PartyName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PartyName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for PartyName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
