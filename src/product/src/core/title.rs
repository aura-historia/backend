use std::fmt::{Debug, Display};
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct Title(String);

impl From<&str> for Title {
    fn from(s: &str) -> Self {
        if s.len() > 128 {
            match s.split_at_checked(128) {
                Some((truncated, _)) => Self(truncated.to_owned()),
                None => Self(s.to_owned()),
            }
        } else {
            Title(s.to_owned())
        }
    }
}

impl Display for Title {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Title {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Title> for String {
    fn from(t: Title) -> Self {
        t.0
    }
}

impl Deref for Title {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Title {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Title {
    fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
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
    use crate::core::title::Title;
    use fake::{Fake, Faker};

    #[test]
    fn should_fake_title() {
        let _ = Faker.fake::<Title>();
    }
}
