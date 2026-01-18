use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::ops::Deref;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("String is too short for TextQuery, got '{0}', expected at least '{N}'.")]
pub struct TextQueryTooShortError<const N: usize>(usize);

/// A string of at least length N for querying
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct TextQuery<const N: usize>(
    #[cfg_attr(
        feature = "test-data",
        dummy(faker = "fake::faker::lorem::en::Sentence(2..5)")
    )]
    String,
);

impl<const N: usize> TryFrom<&str> for TextQuery<N> {
    type Error = TextQueryTooShortError<N>;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let l = s.len();
        if l < N {
            Err(TextQueryTooShortError(l))
        } else if l >= N && l < 256 {
            Ok(Self(s.into()))
        } else {
            match s.split_at_checked(255) {
                Some((truncated, _)) => Ok(Self(truncated.into())),
                None => Ok(Self(s.into())),
            }
        }
    }
}

impl<const N: usize> Display for TextQuery<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<const N: usize> TryFrom<String> for TextQuery<N> {
    type Error = TextQueryTooShortError<N>;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl<const N: usize> From<TextQuery<N>> for String {
    fn from(t: TextQuery<N>) -> Self {
        t.0
    }
}

impl<const N: usize> Deref for TextQuery<N> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> AsRef<str> for TextQuery<N> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
