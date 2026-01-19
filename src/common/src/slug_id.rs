use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlugId(String);

impl SlugId {
    pub fn raw<S: AsRef<str>>(s: S) -> Self {
        Self(s.as_ref().to_owned())
    }
}

impl Display for SlugId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<SlugId> for String {
    fn from(id: SlugId) -> Self {
        id.0
    }
}

impl From<String> for SlugId {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&String> for SlugId {
    fn from(value: &String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&str> for SlugId {
    fn from(value: &str) -> Self {
        let slug = slug::slugify(value);
        let uuid = Uuid::new_v4().to_string();
        let unique_suffix = uuid
            .split('-')
            .next()
            .expect("shouldn't fail splitting off first UUIDv4 segment");
        SlugId(format!("{slug}-{unique_suffix}"))
    }
}

#[cfg(test)]
mod tests {
    use crate::slug_id::SlugId;

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
    fn should_make_slug_id(#[case] text: &str, #[case] expected_slug: &str) {
        let actual = SlugId::from(text);
        assert!(actual.0.starts_with(expected_slug));
    }
}
