use domain_primitives::string_newtype;

string_newtype!(
    Title,
    struct_only,
    derives(serde::Serialize, serde::Deserialize)
);

impl Title {
    pub const MAX_CHARS: usize = 128;
}

impl From<&str> for Title {
    fn from(s: &str) -> Self {
        let mut s = s.trim();
        // Keep ellipses, including shortened ellipses in previously persisted titles.
        // Removing sentence punctuation can expose whitespace or another sentence period.
        while !s.ends_with("..") {
            let Some(without_period) = s.strip_suffix('.') else {
                break;
            };
            s = without_period.trim_end();
        }

        let mut chars = s.chars();
        let capitalized: String = match chars.next() {
            None => String::new(),
            Some(first) => {
                let first_upper: String = first.to_uppercase().collect();
                first_upper + chars.as_str()
            }
        };

        const ELLIPSIS: &str = "...";
        const ELLIPSIS_CHAR_LEN: usize = 3;

        if capitalized.chars().count() > Self::MAX_CHARS {
            let truncated: String = capitalized
                .chars()
                .take(Self::MAX_CHARS - ELLIPSIS_CHAR_LEN)
                .collect();
            Title(truncated + ELLIPSIS)
        } else {
            Title(capitalized)
        }
    }
}

impl From<String> for Title {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<&String> for Title {
    fn from(s: &String) -> Self {
        Self::from(s.as_str())
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Title {
    fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        use fake::Fake;
        let paragraphs: Vec<String> = fake::faker::lorem::en::Words(2..10).fake_with_rng(rng);
        Self(paragraphs.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use crate::title::Title;

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_title() {
        use fake::{Fake, Faker};

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
    fn should_truncate_long_title_to_max_length() {
        let long_title = "a".repeat(200);
        let title = Title::from(long_title.as_str());
        assert_eq!(title.as_ref().len(), 128);
        assert!(title.as_ref().ends_with("..."));
    }

    #[test]
    fn should_append_ellipsis_when_truncating_title() {
        let long_title = "a".repeat(200);
        let title = Title::from(long_title.as_str());
        // first 'a' is capitalised to 'A', then 124 lowercase 'a's, then "..."
        let expected = format!("A{}...", "a".repeat(124));
        assert_eq!(title.as_ref(), expected);
    }

    #[test]
    fn should_not_append_ellipsis_when_within_limit_for_title() {
        let short_title = "a".repeat(128);
        let title = Title::from(short_title.as_str());
        assert!(!title.as_ref().ends_with("..."));
        assert_eq!(title.as_ref().chars().count(), 128);
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

    #[test]
    fn should_preserve_ellipsis_on_repeated_normalization() {
        for raw in [
            "a".repeat(200),
            "ä".repeat(200),
            "Title...".to_owned(),
            "Title..".to_owned(),
        ] {
            let title = Title::from(raw.as_str());
            assert_eq!(title, Title::from(title.as_ref()));
        }
    }

    #[test]
    fn should_trim_whitespace_exposed_by_terminal_periods() {
        for raw in [
            "hello world .",
            "hello world . .",
            "hello world\t.",
            "hello world... .",
        ] {
            let title = Title::from(raw);
            assert_eq!(title.as_ref(), title.as_ref().trim());
            assert_eq!(title, Title::from(title.as_ref()));
        }
        assert_eq!(Title::from("hello world .").as_ref(), "Hello world");
        assert_eq!(Title::from("hello world... .").as_ref(), "Hello world...");
    }

    #[test]
    fn should_capitalize_after_stripping_period() {
        let title = Title::from("hello world.");
        assert_eq!(title.as_ref(), "Hello world");
    }

    #[test]
    fn should_convert_to_string() {
        let title = Title::from("Antique vase");
        let s: String = title.into();
        assert_eq!(s, "Antique vase");
    }

    #[test]
    fn should_create_from_owned_string() {
        let title = Title::from("Antique vase".to_string());
        assert_eq!(title.as_ref(), "Antique vase");
    }

    #[test]
    fn should_display_title() {
        let title = Title::from("Antique vase");
        assert_eq!(format!("{title}"), "Antique vase");
    }
}
