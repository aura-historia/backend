use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

/// Kebab-Case slug for any given String with a random hex-suffix of length N
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlugId<const N: usize>(String);

impl<const N: usize> SlugId<N> {
    pub fn raw<S: AsRef<str>>(s: S) -> Self {
        Self(s.as_ref().to_owned())
    }
}

impl<const N: usize> Display for SlugId<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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
        SlugId(format!(
            "{slug}{}",
            (N > 0)
                .then_some(format!("-{unique_suffix}"))
                .unwrap_or_default()
        ))
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
}
