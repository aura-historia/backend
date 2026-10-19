use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::ops::Deref;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct UserSearchFilterName(String);

pub const MAX_USER_SEARCH_FILTER_NAME_CHARS: usize = 255;

impl From<&str> for UserSearchFilterName {
    fn from(value: &str) -> Self {
        let end = value
            .char_indices()
            .nth(MAX_USER_SEARCH_FILTER_NAME_CHARS)
            .map_or(value.len(), |(end, _)| end);
        Self(value[..end].to_owned())
    }
}

impl From<String> for UserSearchFilterName {
    fn from(mut value: String) -> Self {
        if let Some((end, _)) = value.char_indices().nth(MAX_USER_SEARCH_FILTER_NAME_CHARS) {
            value.truncate(end);
        }
        Self(value)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_name_at_max_length() {
        let name = UserSearchFilterName::from("a".repeat(255));

        assert_eq!(255, name.chars().count());
    }

    #[test]
    fn should_truncate_name_to_max_length() {
        let name = UserSearchFilterName::from("a".repeat(300));

        assert_eq!(255, name.chars().count());
    }

    #[test]
    fn should_keep_multibyte_name_at_max_length() {
        let value = "ü".repeat(255);

        let name = UserSearchFilterName::from(value.as_str());

        assert_eq!(value, name.as_ref());
    }

    #[test]
    fn should_truncate_multibyte_name_on_character_boundary() {
        let name = UserSearchFilterName::from("ü".repeat(300));

        assert_eq!("ü".repeat(255), name.as_ref());
    }
}
