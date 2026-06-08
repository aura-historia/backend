use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use uuid::Uuid;

/// Kebab-Case slug for any given String with a random hex-suffix of length N
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct SlugId<const N: usize>(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid slug id '{value}' for suffix length {suffix_length}.")]
pub struct InvalidSlugId {
    value: String,
    suffix_length: usize,
}

impl<const N: usize> SlugId<N> {
    pub fn raw<S: AsRef<str>>(s: S) -> Result<Self, InvalidSlugId> {
        let value = s.as_ref();
        validate_slug_id::<N>(value)?;
        Ok(Self(value.to_owned()))
    }
}

impl<const N: usize> Display for SlugId<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<const N: usize> Serialize for SlugId<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const N: usize> Deserialize<'de> for SlugId<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::raw(value).map_err(serde::de::Error::custom)
    }
}

impl<const N: usize> From<SlugId<N>> for String {
    fn from(id: SlugId<N>) -> Self {
        id.0
    }
}

impl<const N: usize> From<String> for SlugId<N> {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl<const N: usize> From<&String> for SlugId<N> {
    fn from(value: &String) -> Self {
        Self::from(value.as_str())
    }
}

impl<const N: usize> From<&str> for SlugId<N> {
    fn from(value: &str) -> Self {
        let slug = slug::slugify(value);
        let uuid = Uuid::new_v4().to_string();
        let unique_suffix = uuid
            .chars()
            .filter(|c| c != &'-')
            .take(N)
            .collect::<String>();

        let value = if N > 0 {
            if slug.is_empty() {
                unique_suffix
            } else {
                format!("{slug}-{unique_suffix}")
            }
        } else {
            slug
        };

        SlugId(value)
    }
}

impl<const N: usize> AsRef<str> for SlugId<N> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> Deref for SlugId<N> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn validate_slug_id<const N: usize>(value: &str) -> Result<(), InvalidSlugId> {
    if value.is_empty() {
        return Ok(());
    }

    let is_valid = if N == 0 {
        is_valid_slug_body(value)
    } else if let Some((slug, suffix)) = value.rsplit_once('-') {
        is_valid_slug_body(slug)
            && suffix.len() == N
            && suffix
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    } else {
        false
    };

    if is_valid {
        Ok(())
    } else {
        Err(InvalidSlugId {
            value: value.to_owned(),
            suffix_length: N,
        })
    }
}

fn is_valid_slug_body(value: &str) -> bool {
    !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

#[macro_export]
macro_rules! slug_id_newtype {
    ($name:ident, $suffix_length:expr) => {
        #[cfg_attr(feature = "test-data", derive(fake::Dummy))]
        #[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name($crate::slug_id::SlugId<$suffix_length>);

        impl $name {
            pub fn raw<S: AsRef<str>>(s: S) -> Result<Self, $crate::slug_id::InvalidSlugId> {
                $crate::slug_id::SlugId::raw(s).map(Self)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0.into()
            }
        }

        impl From<$crate::slug_id::SlugId<$suffix_length>> for $name {
            fn from(value: $crate::slug_id::SlugId<$suffix_length>) -> Self {
                Self(value)
            }
        }

        impl From<$name> for $crate::slug_id::SlugId<$suffix_length> {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value.as_str().into())
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self(value.as_str().into())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.0.as_ref()
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                self.0.as_ref()
            }
        }
    };
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::slug_id::SlugId;
    use fake::{Fake, Faker};

    impl<const N: usize> fake::Dummy<Faker> for SlugId<N> {
        fn dummy_with_rng<R: fake::rand::prelude::RngExt + ?Sized>(
            config: &Faker,
            rng: &mut R,
        ) -> Self {
            let random_string: String = fake::vec![char; 7].iter().collect();
            SlugId::from(format!(
                "{}{}",
                random_string,
                config.fake_with_rng::<String, _>(rng)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::slug_id::SlugId;

    slug_id_newtype!(TestSlugId, 0);

    #[rstest::rstest]
    #[case(
        "Musealer Kabinettschrank 18./19. Jahrhundert",
        "musealer-kabinettschrank-18-19-jahrhundert"
    )]
    #[case(
        "Biedermeier Polsterstuhl Kirschbaum um 1830 Art.Nr. 8241",
        "biedermeier-polsterstuhl-kirschbaum-um-1830-art-nr-8241"
    )]
    #[trace]
    fn should_make_slug_id_for_random_suffix_6(#[case] text: &str, #[case] expected_slug: &str) {
        let actual: SlugId<6> = SlugId::from(text);
        assert!(actual.0.starts_with(&format!("{expected_slug}-")));
    }

    #[rstest::rstest]
    #[case(
        "Musealer Kabinettschrank 18./19. Jahrhundert",
        "musealer-kabinettschrank-18-19-jahrhundert"
    )]
    #[case(
        "Biedermeier Polsterstuhl Kirschbaum um 1830 Art.Nr. 8241",
        "biedermeier-polsterstuhl-kirschbaum-um-1830-art-nr-8241"
    )]
    #[trace]
    fn should_make_slug_id_for_empty_suffix(#[case] text: &str, #[case] expected_slug: &str) {
        let actual: SlugId<0> = SlugId::from(text);
        assert_eq!(expected_slug, &actual.0);
    }

    #[rstest::rstest]
    #[case(
        "Musealer Kabinettschrank 18./19. Jahrhundert",
        "musealer-kabinettschrank-18-19-jahrhundert"
    )]
    #[case(
        "Biedermeier Polsterstuhl Kirschbaum um 1830 Art.Nr. 8241",
        "biedermeier-polsterstuhl-kirschbaum-um-1830-art-nr-8241"
    )]
    #[trace]
    fn should_make_slug_suffix_exactly_of_length_8(
        #[case] text: &str,
        #[case] expected_slug: &str,
    ) {
        let actual: SlugId<8> = SlugId::from(text);
        let actual_str = actual.to_string();
        let suffix = actual_str
            .strip_prefix(&format!("{expected_slug}-"))
            .unwrap();

        assert_eq!(8, suffix.len());
    }

    #[test]
    fn should_accept_valid_raw_slug_id() {
        let actual = SlugId::<6>::raw("my-shop-1a2b3c");
        assert_eq!(actual.unwrap().as_ref(), "my-shop-1a2b3c");
    }

    #[test]
    fn should_reject_invalid_raw_slug_id() {
        let actual = SlugId::<0>::raw("REF: JW150 / LA572845");
        assert!(actual.is_err());
    }

    #[test]
    fn should_generate_strong_slug_newtype() {
        let actual = TestSlugId::from("My test slug");
        assert_eq!(actual.as_ref(), "my-test-slug");
    }
}
