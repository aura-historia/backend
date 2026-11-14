use crate::bounded_text::bounded_text_newtype;
use localization::Language;
use serde_email::Email;

bounded_text_newtype!(
    /// The name a submitter gives: trimmed, 1–200 characters, no control characters.
    SubmitterName,
    max_chars(200),
    allowed_controls(&[])
);

/// The notifier's email address: a bare address in the existing `serde_email::Email`
/// convention, at most 320 bytes. Display text such as `Jane <jane@example.com>` is rejected,
/// so every notice from one address shares one lowercased key for the submission limits.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SubmitterEmail(Email);

/// Why an email address was rejected. It never repeats the address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubmitterEmailError {
    #[error("email address is invalid")]
    Invalid,
    #[error("email address must be a bare address without display text, spaces or controls")]
    NotBareAddress,
    #[error("email address must not exceed {max_bytes} bytes")]
    TooLong { max_bytes: usize },
}

impl SubmitterEmail {
    pub const MAX_BYTES: usize = 320;

    pub fn as_email(&self) -> &Email {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<&str> for SubmitterEmail {
    type Error = SubmitterEmailError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.len() > Self::MAX_BYTES {
            return Err(SubmitterEmailError::TooLong {
                max_bytes: Self::MAX_BYTES,
            });
        }
        if value.chars().any(|character| {
            matches!(character, '<' | '>') || character.is_whitespace() || character.is_control()
        }) {
            return Err(SubmitterEmailError::NotBareAddress);
        }
        // The email error repeats the address, so it is not kept as the cause.
        Email::new(value)
            .map(Self)
            .map_err(|_| SubmitterEmailError::Invalid)
    }
}

impl TryFrom<String> for SubmitterEmail {
    type Error = SubmitterEmailError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl std::fmt::Debug for SubmitterEmail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("SubmitterEmail")
            .field(&"<redacted>")
            .finish()
    }
}

/// The notifier's identity and contact as given in the notice. It is personal data; `Debug`
/// output only shows the language.
#[derive(Debug, Clone, PartialEq)]
pub struct Submitter {
    pub name: SubmitterName,
    pub email: SubmitterEmail,
    pub language: Language,
}

#[cfg(test)]
mod tests {
    use super::{Submitter, SubmitterEmail, SubmitterEmailError, SubmitterName};
    use crate::bounded_text::BoundedTextError;
    use localization::Language;

    #[test]
    fn should_accept_name_at_length_bounds() {
        assert_eq!("J", SubmitterName::try_from(" J ").unwrap().as_str());
        let longest = "n".repeat(SubmitterName::MAX_CHARS);
        assert_eq!(
            longest,
            SubmitterName::try_from(longest.as_str()).unwrap().as_str()
        );
    }

    #[test]
    fn should_reject_blank_or_too_long_name() {
        assert_eq!(Err(BoundedTextError::Blank), SubmitterName::try_from("   "));
        assert_eq!(
            Err(BoundedTextError::TooLong {
                max_chars: 200,
                actual_chars: 201,
            }),
            SubmitterName::try_from("n".repeat(201))
        );
    }

    #[test]
    fn should_reject_control_characters_in_name() {
        for name in ["Jane\nDoe", "Jane\tDoe", "Jane\u{0}Doe", "Jane\u{85}Doe"] {
            assert_eq!(
                Err(BoundedTextError::ControlCharacter),
                SubmitterName::try_from(name)
            );
        }
    }

    #[test]
    fn should_not_reveal_personal_data_in_debug_output() {
        let submitter = Submitter {
            name: SubmitterName::try_from("Jane Doe").unwrap(),
            email: SubmitterEmail::try_from("jane@example.com").unwrap(),
            language: Language::De,
        };

        let debug = format!("{submitter:?}");

        assert!(!debug.contains("Jane"));
        assert!(!debug.contains("jane@example.com"));
        assert!(debug.contains("De"));
    }

    #[test]
    fn should_accept_bare_address_and_keep_its_case() {
        assert_eq!(
            "Jane.Doe@Example.com",
            SubmitterEmail::try_from(" Jane.Doe@Example.com ")
                .unwrap()
                .as_str()
        );
    }

    #[test]
    fn should_reject_display_text_whitespace_and_controls() {
        for email in [
            "Jane <jane@example.com>",
            "<jane@example.com>",
            "\"jane doe\"@example.com",
            "jane\u{7}@example.com",
        ] {
            assert_eq!(
                Err(SubmitterEmailError::NotBareAddress),
                SubmitterEmail::try_from(email),
                "{email:?}"
            );
        }
    }

    #[test]
    fn should_reject_invalid_address() {
        assert_eq!(
            Err(SubmitterEmailError::Invalid),
            SubmitterEmail::try_from("not-an-address")
        );
    }

    #[test]
    fn should_reject_address_longer_than_320_bytes() {
        let too_long = format!("{}@example.com", "a".repeat(SubmitterEmail::MAX_BYTES));

        assert_eq!(
            Err(SubmitterEmailError::TooLong { max_bytes: 320 }),
            SubmitterEmail::try_from(too_long)
        );
    }
}
