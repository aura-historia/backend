/// Generates a UUID v4 newtype struct with standard derives and conversions.
///
/// # Example
/// ```rust
/// use common::uuid_v4_newtype;
/// uuid_v4_newtype!(MyId);
/// ```
#[macro_export]
macro_rules! uuid_v4_newtype {
    ($name:ident) => {
        #[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(into = "String", try_from = "String")]
        pub struct $name(::uuid::Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(::uuid::Uuid::new_v4())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<::uuid::Uuid> for $name {
            fn from(uuid: ::uuid::Uuid) -> Self {
                $name(uuid)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ::uuid::Error;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(&s).map(Self)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0.to_string()
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ::uuid::Error;
            fn try_from(s: &str) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(s).map(Self)
            }
        }

        impl TryFrom<&String> for $name {
            type Error = ::uuid::Error;
            fn try_from(s: &String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(s).map(Self)
            }
        }
    };
}

/// Generates a UUID v7 newtype struct with standard derives and conversions.
///
/// # Example
/// ```rust
/// use common::uuid_v7_newtype;
/// uuid_v7_newtype!(MyEventId);
/// ```
#[macro_export]
macro_rules! uuid_v7_newtype {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(into = "String", try_from = "String")]
        pub struct $name(::uuid::Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(::uuid::Uuid::now_v7())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<::uuid::Uuid> for $name {
            fn from(uuid: ::uuid::Uuid) -> Self {
                $name(uuid)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ::uuid::Error;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(&s).map(Self)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0.to_string()
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ::uuid::Error;
            fn try_from(s: &str) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(s).map(Self)
            }
        }

        impl TryFrom<&String> for $name {
            type Error = ::uuid::Error;
            fn try_from(s: &String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(s).map(Self)
            }
        }

        #[cfg(feature = "test-data")]
        impl<T> ::fake::Dummy<T> for $name {
            fn dummy_with_rng<R: ::fake::RngExt + ?Sized>(_config: &T, _rng: &mut R) -> Self {
                Self::new()
            }
        }
    };
}
