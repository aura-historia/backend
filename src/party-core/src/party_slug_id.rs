use std::{fmt::Display, ops::Deref};

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid party slug '{value}'")]
pub struct InvalidPartySlugId {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartySlugId(String);

impl PartySlugId {
    pub fn raw(value: impl AsRef<str>) -> Result<Self, InvalidPartySlugId> {
        let value = value.as_ref();
        if !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--")
            && value.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidPartySlugId {
                value: value.to_owned(),
            })
        }
    }
}

impl From<&str> for PartySlugId {
    fn from(value: &str) -> Self {
        Self(slug::slugify(value))
    }
}

impl Display for PartySlugId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PartySlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for PartySlugId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
