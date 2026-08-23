use std::{fmt::Display, ops::Deref};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ShopSlugId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid shop slug '{value}'")]
pub struct InvalidShopSlugId {
    value: String,
}

impl ShopSlugId {
    pub fn raw<S: AsRef<str>>(value: S) -> Result<Self, InvalidShopSlugId> {
        let value = value.as_ref();
        if value.is_empty() || is_valid_slug(value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidShopSlugId {
                value: value.to_owned(),
            })
        }
    }
}

impl Display for ShopSlugId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<ShopSlugId> for String {
    fn from(value: ShopSlugId) -> Self {
        value.0
    }
}

impl From<String> for ShopSlugId {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&String> for ShopSlugId {
    fn from(value: &String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&str> for ShopSlugId {
    fn from(value: &str) -> Self {
        Self(slug::slugify(value))
    }
}

impl AsRef<str> for ShopSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for ShopSlugId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn is_valid_slug(value: &str) -> bool {
    !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

#[cfg(test)]
mod tests {
    use super::ShopSlugId;

    #[test]
    fn should_build_slug_from_name() {
        assert_eq!(
            "antik-und-stil",
            ShopSlugId::from("Antik und Stil").as_ref()
        );
    }

    #[test]
    fn should_reject_invalid_raw_slug() {
        assert!(ShopSlugId::raw("Bad Slug").is_err());
    }
}
