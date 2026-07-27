#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidVersionError {
    #[error("version must be greater than zero")]
    Zero,
}

#[macro_export]
macro_rules! version_newtype {
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
        #[serde(try_from = "u64", into = "u64")]
        pub struct $name(u64);

        impl Default for $name {
            fn default() -> Self {
                Self::INITIAL
            }
        }

        impl $name {
            pub const INITIAL: Self = Self(1);

            pub fn into_inner(self) -> u64 {
                self.0
            }

            pub fn next(self) -> Self {
                Self(self.0 + 1)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl TryFrom<u64> for $name {
            type Error = $crate::version::InvalidVersionError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                if value == 0 {
                    Err($crate::version::InvalidVersionError::Zero)
                } else {
                    Ok(Self(value))
                }
            }
        }

        impl TryFrom<i64> for $name {
            type Error = $crate::version::InvalidVersionError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                let unsigned =
                    u64::try_from(value).map_err(|_| $crate::version::InvalidVersionError::Zero)?;
                Self::try_from(unsigned)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        #[cfg(feature = "test-data")]
        impl<T> ::fake::Dummy<T> for $name {
            fn dummy_with_rng<R: ::fake::RngExt + ?Sized>(_config: &T, _rng: &mut R) -> Self {
                Self::INITIAL
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    version_newtype!(TestVersion);

    #[test]
    fn should_create_initial_version() {
        assert_eq!(1, TestVersion::INITIAL.into_inner());
    }

    #[test]
    fn should_increment_version() {
        assert_eq!(2, TestVersion::INITIAL.next().into_inner());
    }

    #[test]
    fn should_reject_zero_version() {
        assert_eq!(
            InvalidVersionError::Zero,
            TestVersion::try_from(0_u64).unwrap_err()
        );
    }

    #[test]
    fn should_reject_negative_version() {
        assert_eq!(
            InvalidVersionError::Zero,
            TestVersion::try_from(-1_i64).unwrap_err()
        );
    }
}
