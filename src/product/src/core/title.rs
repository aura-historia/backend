use std::fmt::{Debug, Display};
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct Title(String);

impl From<&str> for Title {
    fn from(s: &str) -> Self {
        let s = s.trim();
        let s = s.strip_suffix(".").unwrap_or(s);
        let s = s
            .chars()
            .take(128)
            .enumerate()
            .map(|(i, c)| {
                if i == 0 && c.is_lowercase() {
                    c.to_uppercase()
                        .next()
                        .expect("shouldn't fail finding next char in uppercase-string of length 1")
                } else {
                    c
                }
            })
            .collect::<String>();

        Title(s)
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
        let paragraphs: Vec<String> = fake::faker::lorem::en::Words(2..10).fake_with_rng(rng);
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

    #[test]
    fn should_trim_title() {
        let title = Title::from("   Hello World   ");
        assert_eq!(title.as_ref(), "Hello World");
    }

    #[test]
    fn should_capitalize_first_letter() {
        let title = Title::from("hello world");
        assert_eq!(title.as_ref(), "Hello world");
    }

    #[test]
    fn should_truncate_long_title() {
        let long_title = "a".repeat(200);
        let title = Title::from(long_title.as_str());
        assert_eq!(title.as_ref().len(), 128);
    }

    #[test]
    fn should_handle_empty_title() {
        let title = Title::from("");
        assert_eq!(title.as_ref(), "");
    }

    #[test]
    fn should_handle_title_with_period() {
        let title = Title::from("Hello World.");
        assert_eq!(title.as_ref(), "Hello World");
    }
}
