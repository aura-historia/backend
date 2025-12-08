use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Domain(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("URL '{0}' does not contain a domain")]
pub struct NoDomainError(String);

impl Domain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&Url> for Domain {
    type Error = NoDomainError;

    fn try_from(url: &Url) -> Result<Self, Self::Error> {
        match url.domain() {
            Some(domain) => Ok(Domain(domain.to_lowercase())),
            None => Err(NoDomainError(url.to_string())),
        }
    }
}

impl TryFrom<Url> for Domain {
    type Error = NoDomainError;

    fn try_from(url: Url) -> Result<Self, Self::Error> {
        Self::try_from(&url)
    }
}

impl TryFrom<&str> for Domain {
    type Error = NoDomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let domain = value
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("www.")
            .chars()
            .take_while(|char| char != &':' && char != &'/' && char != &'?' && char != &'#')
            .collect::<String>()
            .to_lowercase();

        if domain.is_empty() || !domain.contains('.') {
            Err(NoDomainError(domain))
        } else {
            Ok(Domain(domain))
        }
    }
}

impl TryFrom<String> for Domain {
    type Error = NoDomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<Domain> for String {
    fn from(domain: Domain) -> Self {
        domain.0
    }
}

impl Display for Domain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng, faker::internet::en::DomainSuffix};

    impl Dummy<Faker> for Domain {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let second_level_domain = config.fake_with_rng::<String, R>(rng);
            let top_level_domain: String = DomainSuffix().fake_with_rng(rng);
            let subdomain = if config.fake_with_rng(rng) {
                format!("{}.", config.fake_with_rng::<String, R>(rng))
            } else {
                "".to_owned()
            };

            Domain(format!(
                "{subdomain}{second_level_domain}.{top_level_domain}"
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::domain::Domain;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_domain() {
            let s = Faker.fake::<Domain>();
            println!("{s}")
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::Domain;

    #[rstest::rstest]
    #[case("foo.bar", "foo.bar")]
    #[case("foo.bar.baz", "foo.bar.baz")]
    #[case("foo.bar.baz.bat", "foo.bar.baz.bat")]
    #[case("www.foo.bar", "foo.bar")]
    #[case("www.foo.bar.baz", "foo.bar.baz")]
    #[case("www.foo.bar.baz.bat", "foo.bar.baz.bat")]
    #[case("http://foo.bar", "foo.bar")]
    #[case("http://foo.bar.baz", "foo.bar.baz")]
    #[case("http://foo.bar.baz.bat", "foo.bar.baz.bat")]
    #[case("https://foo.bar", "foo.bar")]
    #[case("https://foo.bar.baz", "foo.bar.baz")]
    #[case("https://foo.bar.baz.bat", "foo.bar.baz.bat")]
    #[case("https://foo.bar/boop", "foo.bar")]
    #[case("https://foo.bar.baz/boop", "foo.bar.baz")]
    #[case("https://foo.bar.baz.bat/boop", "foo.bar.baz.bat")]
    #[case("https://foo.bar/boop/beep", "foo.bar")]
    #[case("https://foo.bar.baz/boop/beep", "foo.bar.baz")]
    #[case("https://foo.bar.baz.bat/boop/beep", "foo.bar.baz.bat")]
    #[case("https://foo.bar/boop?meep=maap", "foo.bar")]
    #[case("https://foo.bar.baz/boop?meep=maap", "foo.bar.baz")]
    #[case("https://foo.bar.baz.bat/boop?meep=maap", "foo.bar.baz.bat")]
    #[case("https://foo.bar/boop?meep=maap&moop=moop", "foo.bar")]
    #[case("https://foo.bar.baz/boop?meep=maap&moop=moop", "foo.bar.baz")]
    #[case("https://foo.bar.baz.bat/boop?meep=maap&moop=moop", "foo.bar.baz.bat")]
    #[case("https://foo.bar/boop?meep=maap&moop=moop#nig", "foo.bar")]
    #[case("https://foo.bar.baz/boop?meep=maap&moop=moop#nig", "foo.bar.baz")]
    #[case(
        "https://foo.bar.baz.bat/boop?meep=maap&moop=moop#nig",
        "foo.bar.baz.bat"
    )]
    fn should_succeed_try_from_url_when_contains_domain(
        #[case] url_str: String,
        #[case] expected_str: String,
    ) {
        let expected = Domain(expected_str);

        let actual = Domain::try_from(url_str).unwrap();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case("https://foo")]
    #[case("https://foo:8080")]
    fn should_fail_try_from_url_when_not_contains_domain(#[case] url_str: String) {
        let actual = Domain::try_from(url_str);

        assert!(actual.is_err());
    }
}
