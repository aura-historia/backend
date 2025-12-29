use crate::core::sanitize::sanitize;
use std::fmt::Display;
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct Description(String);

impl From<&str> for Description {
    fn from(s: &str) -> Self {
        if s.len() > 2000 {
            match s.split_at_checked(2000) {
                Some((truncated, _)) => Self(sanitize(truncated)),
                None => Self(sanitize(s)),
            }
        } else {
            Description(sanitize(s))
        }
    }
}

impl Display for Description {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Description {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Description> for String {
    fn from(t: Description) -> Self {
        t.0
    }
}

impl Deref for Description {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Description {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Description {
    fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        use fake::Fake;

        let paragraphs: Vec<String> = fake::faker::lorem::en::Paragraphs(1..7).fake_with_rng(rng);
        Self::from(paragraphs.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use crate::core::description::Description;
    use fake::{Fake, Faker};

    #[test]
    fn should_fake_description() {
        let _ = Faker.fake::<Description>();
    }
}
