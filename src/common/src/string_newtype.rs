#[macro_export]
macro_rules! string_newtype {
    ($name:ident, max_length($max:expr) $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner $name $(, derives($($derive),*))?);

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
                    match trimmed.split_at_checked($max) {
                        Some((truncated, _)) => $name(truncated.into()),
                        None => $name(trimmed.into()),
                    }
                } else {
                    $name(trimmed.into())
                }
            }
        }
    };

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
    };
}

#[cfg(test)]
mod tests {
    string_newtype!(PlainType);
    string_newtype!(BoundedType, max_length(10));

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
    fn should_create_bounded_type_from_str() {
        let val = BoundedType::from("short");
        assert_eq!(val.as_ref(), "short");
    }

    #[test]
    fn should_truncate_bounded_type_exceeding_max_length() {
        let val = BoundedType::from("this is way too long");
        assert_eq!(val.as_ref().len(), 10);
        assert_eq!(val.as_ref(), "this is wa");
    }

    #[test]
    fn should_trim_whitespace_for_bounded_type() {
        let val = BoundedType::from("  hello  ");
        assert_eq!(val.as_ref(), "hello");
    }

    #[test]
    fn should_trim_then_truncate_for_bounded_type() {
        let val = BoundedType::from("   abcdefghijklm   ");
        assert_eq!(val.as_ref().len(), 10);
        assert_eq!(val.as_ref(), "abcdefghij");
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
    fn should_convert_to_string_for_plain_type() {
        let val = PlainType::from("test");
        let s: String = val.into();
        assert_eq!(s, "test");
    }

    #[test]
    fn should_convert_to_string_for_bounded_type() {
        let val = BoundedType::from("test");
        let s: String = val.into();
        assert_eq!(s, "test");
    }

    #[test]
    fn should_create_bounded_type_from_owned_string() {
        let val = BoundedType::from("this is way too long".to_string());
        assert_eq!(val.as_ref().len(), 10);
    }

    #[test]
    fn should_display_plain_type() {
        let val = PlainType::from("display me");
        assert_eq!(format!("{val}"), "display me");
    }

    #[test]
    fn should_display_bounded_type() {
        let val = BoundedType::from("display");
        assert_eq!(format!("{val}"), "display");
    }
}
