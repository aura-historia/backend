use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct UserSearchFilterName(String);

impl From<&str> for UserSearchFilterName {
    fn from(s: &str) -> Self {
        if s.len() > 255 {
            match s.split_at_checked(255) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            UserSearchFilterName(s.into())
        }
    }
}

impl Display for UserSearchFilterName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for UserSearchFilterName {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<UserSearchFilterName> for String {
    fn from(t: UserSearchFilterName) -> Self {
        t.0
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

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for UserSearchFilterName {
    fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        use fake::Fake;

        fn some_kind_of_uppercase_first_letter(s: &str) -> String {
            let mut c = s.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        }

        let mut paragraphs: Vec<String> = fake::faker::lorem::en::Words(2..10).fake_with_rng(rng);
        paragraphs[0] = some_kind_of_uppercase_first_letter(paragraphs.first().unwrap());

        Self(paragraphs.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::UserSearchFilterName;

    #[test]
    fn should_create_from_str() {
        let name = UserSearchFilterName::from("My Filter");
        assert_eq!(name.as_ref(), "My Filter");
    }

    #[test]
    fn should_truncate_to_255_bytes() {
        let long = "a".repeat(300);
        let name = UserSearchFilterName::from(long.as_str());
        assert_eq!(name.as_ref().len(), 255);
    }

    #[test]
    fn should_not_trim_whitespace() {
        let name = UserSearchFilterName::from("  spaced  ");
        assert_eq!(name.as_ref(), "  spaced  ");
    }

    #[test]
    fn should_round_trip_via_string() {
        let name = UserSearchFilterName::from("My Filter");
        let s: String = name.clone().into();
        let name2 = UserSearchFilterName::from(s);
        assert_eq!(name, name2);
    }

    #[test]
    fn should_display() {
        let name = UserSearchFilterName::from("My Filter");
        assert_eq!(format!("{name}"), "My Filter");
    }
}
