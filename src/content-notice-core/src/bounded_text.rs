/// Why submitted free text was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundedTextError {
    #[error("text must not be blank")]
    Blank,
    #[error("text must not exceed {max_chars} characters (got {actual_chars})")]
    TooLong {
        max_chars: usize,
        actual_chars: usize,
    },
    #[error("text must not contain control characters")]
    ControlCharacter,
}

/// Line breaks and tabs that multi-line text may keep.
pub(crate) const LAYOUT_CHARACTERS: &[char] = &['\n', '\r', '\t'];

/// Trims `value` and checks it is nonblank, at most `max_chars` characters long and free of
/// control characters other than `allowed_controls`. Values are never truncated.
pub(crate) fn validate(
    value: &str,
    max_chars: usize,
    allowed_controls: &[char],
) -> Result<String, BoundedTextError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(BoundedTextError::Blank);
    }
    let actual_chars = value.chars().count();
    if actual_chars > max_chars {
        return Err(BoundedTextError::TooLong {
            max_chars,
            actual_chars,
        });
    }
    if value
        .chars()
        .any(|character| character.is_control() && !allowed_controls.contains(&character))
    {
        return Err(BoundedTextError::ControlCharacter);
    }
    Ok(value.to_owned())
}

/// Defines a validated, trimmed text newtype whose `Debug` output never shows the value, so
/// personal data cannot leak through logged errors or structs.
macro_rules! bounded_text_newtype {
    ($(#[$meta:meta])* $name:ident, max_chars($max:expr), allowed_controls($allowed:expr)) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub const MAX_CHARS: usize = $max;

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<&str> for $name {
            type Error = $crate::bounded_text::BoundedTextError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                $crate::bounded_text::validate(value, Self::MAX_CHARS, $allowed).map(Self)
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::bounded_text::BoundedTextError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::try_from(value.as_str())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter
                    .debug_tuple(::core::stringify!($name))
                    .field(&"<redacted>")
                    .finish()
            }
        }
    };
}

pub(crate) use bounded_text_newtype;

#[cfg(test)]
mod tests {
    use super::{BoundedTextError, LAYOUT_CHARACTERS, validate};

    #[test]
    fn should_trim_surrounding_whitespace() {
        assert_eq!(
            Ok("Jane Doe".to_owned()),
            validate("  Jane Doe \n", 10, &[])
        );
    }

    #[test]
    fn should_reject_blank_text() {
        assert_eq!(Err(BoundedTextError::Blank), validate(" \t\n ", 10, &[]));
    }

    #[test]
    fn should_count_characters_not_bytes() {
        assert_eq!(Ok("ääää".to_owned()), validate("ääää", 4, &[]));
        assert_eq!(
            Err(BoundedTextError::TooLong {
                max_chars: 4,
                actual_chars: 5,
            }),
            validate("äääää", 4, &[])
        );
    }

    #[test]
    fn should_reject_control_characters_unless_allowed() {
        assert_eq!(
            Err(BoundedTextError::ControlCharacter),
            validate("a\nb", 10, &[])
        );
        assert_eq!(
            Ok("a\r\n\tb".to_owned()),
            validate("a\r\n\tb", 10, LAYOUT_CHARACTERS)
        );
        for control in ['\u{0}', '\u{7}', '\u{1b}', '\u{7f}', '\u{85}', '\u{9f}'] {
            assert_eq!(
                Err(BoundedTextError::ControlCharacter),
                validate(&format!("a{control}b"), 10, LAYOUT_CHARACTERS)
            );
        }
    }
}
