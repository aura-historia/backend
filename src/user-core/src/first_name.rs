use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    ops::Deref,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirstName(
    #[cfg_attr(
        feature = "test-data",
        dummy(faker = "fake::faker::name::en::FirstName()")
    )]
    String,
);

impl Display for FirstName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for FirstName {
    fn from(s: &str) -> Self {
        if s.len() > 64 {
            match s.split_at_checked(64) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            FirstName(s.into())
        }
    }
}

impl From<String> for FirstName {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<FirstName> for String {
    fn from(t: FirstName) -> Self {
        t.0
    }
}

impl Deref for FirstName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for FirstName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_first_name_when_at_max_length() {
        let name = FirstName::from("a".repeat(64));

        assert_eq!(64, name.as_ref().len());
    }

    #[test]
    fn should_truncate_first_name_to_max_length() {
        let name = FirstName::from("a".repeat(80));

        assert_eq!(64, name.as_ref().len());
    }

    #[test]
    fn should_keep_first_name_when_split_point_is_not_char_boundary() {
        let input = format!("{}é", "a".repeat(63));
        let name = FirstName::from(input.clone());

        assert_eq!(input, name.as_ref());
    }

    #[test]
    fn should_convert_first_name_to_string() {
        let name = FirstName::from("Ada");

        assert_eq!("Ada", name.to_string());
        assert_eq!("Ada", String::from(name));
    }
}
