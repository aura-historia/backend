use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    ops::Deref,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Name(
    #[cfg_attr(feature = "test-data", dummy(faker = "fake::faker::name::en::Name()"))] String,
);

impl Display for Name {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        if s.len() > 128 {
            match s.split_at_checked(128) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            Name(s.into())
        }
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Name> for String {
    fn from(t: Name) -> Self {
        t.0
    }
}

impl Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_name_when_at_max_length() {
        let name = Name::from("a".repeat(128));

        assert_eq!(128, name.as_ref().len());
    }

    #[test]
    fn should_truncate_name_to_max_length() {
        let name = Name::from("a".repeat(160));

        assert_eq!(128, name.as_ref().len());
    }

    #[test]
    fn should_keep_name_when_split_point_is_not_char_boundary() {
        let input = format!("{}é", "a".repeat(127));
        let name = Name::from(input.clone());

        assert_eq!(input, name.as_ref());
    }

    #[test]
    fn should_convert_name_to_string() {
        let name = Name::from("Ada Lovelace");

        assert_eq!("Ada Lovelace", name.to_string());
        assert_eq!("Ada Lovelace", String::from(name));
    }
}
