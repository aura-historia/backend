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
    use crate::core::user_search_filter_name::UserSearchFilterName;
    use fake::{Fake, Faker};

    #[test]
    fn should_fake_search_filter_name() {
        let _ = Faker.fake::<UserSearchFilterName>();
    }
}
