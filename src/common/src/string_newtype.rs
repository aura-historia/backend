#[macro_export]
macro_rules! string_newtype {
    // Case with serde enabled
    ($name:ident, serde) => {
        #[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
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

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };

    // Case without serde
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
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

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}
