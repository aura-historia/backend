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

    fn try_from(value: &Url) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<Url> for Domain {
    type Error = NoDomainError;

    fn try_from(value: Url) -> Result<Self, Self::Error> {
        Self::try_from(&value)
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
            .take_while(|character| !matches!(character, ':' | '/' | '?' | '#'))
            .collect::<String>()
            .to_lowercase();

        if domain.is_empty() || !domain.contains('.') {
            Err(NoDomainError(domain))
        } else {
            Ok(Self(domain))
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
    fn from(value: Domain) -> Self {
        value.0
    }
}

impl Display for Domain {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt, faker::internet::en::DomainSuffix};

    impl Dummy<Faker> for Domain {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let second_level_domain = config.fake_with_rng::<String, R>(rng);
            let top_level_domain: String = DomainSuffix().fake_with_rng(rng);
            let subdomain = if config.fake_with_rng(rng) {
                format!("{}.", config.fake_with_rng::<String, R>(rng))
            } else {
                String::new()
            };

            Self(format!("{subdomain}{second_level_domain}.{top_level_domain}").to_lowercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Domain;

    #[test]
    fn should_normalize_domain_from_url() {
        let domain = Domain::try_from("https://www.antiquitaeten-tuebingen.de/path");

        assert_eq!(
            Ok("antiquitaeten-tuebingen.de".to_string()),
            domain.map(Into::into)
        );
    }

    #[test]
    fn should_reject_value_without_domain() {
        assert!(Domain::try_from("https://localhost").is_err());
    }
}
