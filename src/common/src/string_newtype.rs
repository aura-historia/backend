#[macro_export]
macro_rules! string_newtype {
    // max_length variant without fake::Dummy auto-derive (for types with a custom Dummy impl)
    ($name:ident, max_length($max:expr), no_fake $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner_no_fake $name $(, derives($($derive),*))?);
        $crate::string_newtype!(@max_length_from $name, $max);
    };

    // max_length variant (standard — with fake::Dummy auto-derive)
    ($name:ident, max_length($max:expr) $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner $name $(, derives($($derive),*))?);
        $crate::string_newtype!(@max_length_from $name, $max);
    };

    // struct_only variant: generates the struct + boilerplate impls, but no From impls and no
    // fake::Dummy auto-derive. Use this for types that need custom From<&str> logic (e.g. Title).
    ($name:ident, struct_only $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner_no_fake $name $(, derives($($derive),*))?);
    };

    // plain variant (no length limit — with fake::Dummy auto-derive)
    ($name:ident $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner $name $(, derives($($derive),*))?);

        impl From<String> for $name {
            fn from(value: String) -> Self {
                $name(value)
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                $name(value.to_owned())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                $name(value.to_owned())
            }
        }
    };

    // ---------------------------------------------------------------------------
    // Internal helpers
    // ---------------------------------------------------------------------------

    // Shared From impls for all max_length variants.
    // When the trimmed input exceeds `$max` bytes the value is truncated and
    // "..." is appended so that the total length stays within `$max` bytes.
    (@max_length_from $name:ident, $max:expr) => {
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::from(value.as_str())
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self::from(value.as_str())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                let trimmed = value.trim();
                if trimmed.len() > $max {
                    // Only append "..." when there is enough room (i.e. $max >= 3 bytes).
                    if $max >= 3 {
                        let truncate_at = $max - 3;
                        match trimmed.split_at_checked(truncate_at) {
                            Some((truncated, _)) => $name(format!("{}...", truncated)),
                            // split_at_checked returns None when the byte index falls inside a
                            // multi-byte character; fall back to the full trimmed value.
                            None => $name(trimmed.into()),
                        }
                    } else {
                        match trimmed.split_at_checked($max) {
                            Some((truncated, _)) => $name(truncated.into()),
                            None => $name(trimmed.into()),
                        }
                    }
                } else {
                    $name(trimmed.into())
                }
            }
        }
    };

    // Struct definition + boilerplate impls WITH fake::Dummy auto-derive.
    (@inner $name:ident $(, derives($($derive:path),* $(,)?))? ) => {
        #[cfg_attr(feature = "test-data", derive(fake::Dummy))]
        #[derive(
            Debug,
            Clone,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash
            $(, $($derive),*)?
        )]
        pub struct $name(String);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
    };

    // Struct definition + boilerplate impls WITHOUT fake::Dummy auto-derive.
    (@inner_no_fake $name:ident $(, derives($($derive:path),* $(,)?))? ) => {
        #[derive(
            Debug,
            Clone,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash
            $(, $($derive),*)?
        )]
        pub struct $name(String);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}

#[cfg(test)]
mod tests {
    string_newtype!(PlainType);
    string_newtype!(BoundedType, max_length(10));

    // -------------------------------------------------------------------------
    // PlainType
    // -------------------------------------------------------------------------

    #[test]
    fn should_create_plain_type_from_str() {
        let val = PlainType::from("hello");
        assert_eq!(val.as_ref(), "hello");
    }

    #[test]
    fn should_not_trim_plain_type() {
        let val = PlainType::from("  spaces  ");
        assert_eq!(val.as_ref(), "  spaces  ");
    }

    #[test]
    fn should_convert_to_string_for_plain_type() {
        let val = PlainType::from("test");
        let s: String = val.into();
        assert_eq!(s, "test");
    }

    #[test]
    fn should_display_plain_type() {
        let val = PlainType::from("display me");
        assert_eq!(format!("{val}"), "display me");
    }

    #[test]
    fn should_deref_plain_type_to_str() {
        let val = PlainType::from("deref me");
        assert!(val.contains("deref"));
    }

    // -------------------------------------------------------------------------
    // BoundedType — values within the limit
    // -------------------------------------------------------------------------

    #[test]
    fn should_create_bounded_type_from_str_when_within_limit() {
        let val = BoundedType::from("short");
        assert_eq!(val.as_ref(), "short");
    }

    #[test]
    fn should_trim_whitespace_for_bounded_type() {
        let val = BoundedType::from("  hello  ");
        assert_eq!(val.as_ref(), "hello");
    }

    #[test]
    fn should_keep_exact_max_length_for_bounded_type() {
        let val = BoundedType::from("1234567890");
        assert_eq!(val.as_ref().len(), 10);
        assert_eq!(val.as_ref(), "1234567890");
    }

    #[test]
    fn should_handle_empty_string_for_bounded_type() {
        let val = BoundedType::from("");
        assert_eq!(val.as_ref(), "");
    }

    #[test]
    fn should_convert_to_string_for_bounded_type() {
        let val = BoundedType::from("test");
        let s: String = val.into();
        assert_eq!(s, "test");
    }

    #[test]
    fn should_display_bounded_type() {
        let val = BoundedType::from("display");
        assert_eq!(format!("{val}"), "display");
    }

    #[test]
    fn should_deref_bounded_type_to_str() {
        let val = BoundedType::from("hello");
        assert!(val.contains("hello"));
    }

    // -------------------------------------------------------------------------
    // BoundedType — truncation with "..."
    // -------------------------------------------------------------------------

    #[test]
    fn should_truncate_and_append_ellipsis_when_exceeding_max_length_for_bounded_type() {
        let val = BoundedType::from("this is way too long");
        assert_eq!(val.as_ref().len(), 10);
        assert_eq!(val.as_ref(), "this is...");
    }

    #[test]
    fn should_trim_then_truncate_with_ellipsis_for_bounded_type() {
        let val = BoundedType::from("   abcdefghijklm   ");
        assert_eq!(val.as_ref().len(), 10);
        assert_eq!(val.as_ref(), "abcdefg...");
    }

    #[test]
    fn should_append_ellipsis_when_truncating_for_bounded_type() {
        let val = BoundedType::from("exceeds max");
        assert!(
            val.as_ref().ends_with("..."),
            "expected '...' suffix, got: {:?}",
            val.as_ref()
        );
    }

    #[test]
    fn should_not_append_ellipsis_when_within_limit_for_bounded_type() {
        let val = BoundedType::from("short");
        assert!(
            !val.as_ref().ends_with("..."),
            "did not expect '...' suffix for value within limit"
        );
    }

    #[test]
    fn should_create_bounded_type_from_owned_string_with_truncation() {
        let val = BoundedType::from("this is way too long".to_string());
        assert_eq!(val.as_ref().len(), 10);
        assert!(val.as_ref().ends_with("..."));
    }

    #[test]
    fn should_create_bounded_type_from_string_ref_with_truncation() {
        let s = "this is way too long".to_string();
        let val = BoundedType::from(&s);
        assert_eq!(val.as_ref().len(), 10);
        assert!(val.as_ref().ends_with("..."));
    }
}
