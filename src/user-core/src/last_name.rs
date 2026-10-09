use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    ops::Deref,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "String")]
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
    fn from(value: &str) -> Self {
        let end = value
            .char_indices()
            .nth(64)
            .map_or(value.len(), |(end, _)| end);
        Self(value[..end].to_owned())
    }
}

impl From<String> for LastName {
    fn from(mut value: String) -> Self {
        if let Some((end, _)) = value.char_indices().nth(64) {
            value.truncate(end);
        }
        Self(value)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_last_name_when_at_max_length() {
        let name = LastName::from("a".repeat(64));

        assert_eq!(64, name.as_ref().len());
    }

    #[test]
    fn should_truncate_last_name_to_max_length() {
        let name = LastName::from("a".repeat(80));

        assert_eq!(64, name.as_ref().len());
    }

    #[test]
    fn should_keep_last_name_when_split_point_is_not_char_boundary() {
        let input = format!("{}é", "a".repeat(63));
        let name = LastName::from(input.clone());

        assert_eq!(input, name.as_ref());
    }

    #[test]
    fn should_convert_last_name_to_string() {
        let name = LastName::from("Lovelace");

        assert_eq!("Lovelace", name.to_string());
        assert_eq!("Lovelace", String::from(name));
    }
    #[test]
    fn deserialization_uses_the_newtype_constructor_for_unicode_and_ascii() {
        for input in [
            "a".repeat(80),
            "é".repeat(80),
            format!("{}é{}", "a".repeat(63), "z".repeat(80)),
        ] {
            let owned = LastName::from(input.clone());
            let borrowed = LastName::from(input.as_str());
            let json = serde_json::to_string(&input).unwrap();
            let decoded: LastName = serde_json::from_str(&json).unwrap();
            assert_eq!(owned, borrowed);
            assert_eq!(owned, decoded);
            assert_eq!(64, decoded.chars().count());
            assert_eq!(
                decoded.as_ref(),
                serde_json::from_str::<String>(&serde_json::to_string(&decoded).unwrap()).unwrap()
            );
        }
    }
}
