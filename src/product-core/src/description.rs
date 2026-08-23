use domain_primitives::string_newtype;

string_newtype!(Description, max_length(4000), no_fake);

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Description {
    fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        use fake::Fake;

        let paragraphs: Vec<String> = fake::faker::lorem::en::Paragraphs(1..7).fake_with_rng(rng);
        Self::from(paragraphs.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use crate::description::Description;

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_description() {
        use fake::{Fake, Faker};

        let _ = Faker.fake::<Description>();
    }

    #[test]
    fn should_truncate_description_to_max_length() {
        let long_string = "a".repeat(5000);
        let description = Description::from(long_string.as_str());
        assert_eq!(description.len(), 4000);
    }

    #[test]
    fn should_append_ellipsis_when_truncating_description() {
        let long_string = "a".repeat(5000);
        let description = Description::from(long_string.as_str());
        assert_eq!(description.as_ref(), &format!("{}...", "a".repeat(3997)));
    }

    #[test]
    fn should_not_truncate_description_within_limit() {
        let short_string = "a".repeat(4000);
        let description = Description::from(short_string.as_str());
        assert_eq!(description.as_ref(), short_string);
    }

    #[test]
    fn should_handle_empty_description() {
        let description = Description::from("");
        assert_eq!(description.as_ref(), "");
    }

    #[test]
    fn should_handle_whitespace_description() {
        let description = Description::from("   ");
        assert_eq!(description.as_ref(), "");
    }

    #[test]
    fn should_trim_description() {
        let description = Description::from("   Hello, World!   ");
        assert_eq!(description.as_ref(), "Hello, World!");
    }
}
