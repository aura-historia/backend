use url::Url;

/// A further location the submitter points to: an absolute `http` or `https` URL without
/// credentials and at most 2048 characters long, kept in the form the `url` crate normalizes
/// it to. Submitted URLs are personal content, so `Debug` never shows them.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AdditionalUrl(Url);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdditionalUrlError {
    #[error("additional URL must not exceed {max_chars} characters (got {actual_chars})")]
    TooLong {
        max_chars: usize,
        actual_chars: usize,
    },
    #[error("additional URL is not an absolute URL")]
    Invalid(#[source] url::ParseError),
    #[error("additional URL must use http or https")]
    UnsupportedScheme,
    #[error("additional URL must not contain credentials")]
    Credentials,
}

impl AdditionalUrl {
    pub const MAX_CHARS: usize = 2048;

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn check_length(value: &str) -> Result<(), AdditionalUrlError> {
        let actual_chars = value.chars().count();
        if actual_chars > Self::MAX_CHARS {
            return Err(AdditionalUrlError::TooLong {
                max_chars: Self::MAX_CHARS,
                actual_chars,
            });
        }
        Ok(())
    }
}

impl TryFrom<&str> for AdditionalUrl {
    type Error = AdditionalUrlError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        Self::check_length(value)?;
        let url = Url::parse(value).map_err(AdditionalUrlError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AdditionalUrlError::UnsupportedScheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(AdditionalUrlError::Credentials);
        }
        // Normalization can lengthen a URL, for example by percent-encoding; bound what is kept.
        Self::check_length(url.as_str())?;
        Ok(Self(url))
    }
}

impl TryFrom<String> for AdditionalUrl {
    type Error = AdditionalUrlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl std::fmt::Debug for AdditionalUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("AdditionalUrl")
            .field(&"<redacted>")
            .finish()
    }
}

/// The additional URLs of one notice: at most ten, none repeated after normalization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdditionalUrls(Vec<AdditionalUrl>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdditionalUrlsError {
    #[error("at most {max_items} additional URLs are allowed (got {actual_items})")]
    TooMany {
        max_items: usize,
        actual_items: usize,
    },
    #[error("additional URL at position {index} repeats an earlier one")]
    Duplicate { index: usize },
}

impl AdditionalUrls {
    pub const MAX_ITEMS: usize = 10;

    pub fn as_slice(&self) -> &[AdditionalUrl] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl TryFrom<Vec<AdditionalUrl>> for AdditionalUrls {
    type Error = AdditionalUrlsError;

    fn try_from(urls: Vec<AdditionalUrl>) -> Result<Self, Self::Error> {
        if urls.len() > Self::MAX_ITEMS {
            return Err(AdditionalUrlsError::TooMany {
                max_items: Self::MAX_ITEMS,
                actual_items: urls.len(),
            });
        }
        if let Some(index) = (1..urls.len()).find(|&index| urls[..index].contains(&urls[index])) {
            return Err(AdditionalUrlsError::Duplicate { index });
        }
        Ok(Self(urls))
    }
}

#[cfg(test)]
mod tests {
    use super::{AdditionalUrl, AdditionalUrlError, AdditionalUrls, AdditionalUrlsError};

    fn url(value: &str) -> AdditionalUrl {
        AdditionalUrl::try_from(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    #[test]
    fn should_accept_http_and_https_urls() {
        assert_eq!(
            "http://example.com/a",
            url(" http://example.com/a ").as_str()
        );
        assert_eq!(
            "https://example.com/photo?id=1",
            url("https://example.com/photo?id=1").as_str()
        );
    }

    #[test]
    fn should_reject_relative_or_malformed_urls() {
        for value in ["/relative/path", "example.com", "https://", ""] {
            assert!(
                matches!(
                    AdditionalUrl::try_from(value),
                    Err(AdditionalUrlError::Invalid(_))
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn should_reject_other_schemes() {
        for value in [
            "ftp://example.com/file",
            "mailto:jane@example.com",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            assert_eq!(
                Err(AdditionalUrlError::UnsupportedScheme),
                AdditionalUrl::try_from(value),
                "{value}"
            );
        }
    }

    #[test]
    fn should_reject_userinfo() {
        for value in [
            "https://user@example.com/",
            "https://user:secret@example.com/",
            "https://:secret@example.com/",
        ] {
            assert_eq!(
                Err(AdditionalUrlError::Credentials),
                AdditionalUrl::try_from(value),
                "{value}"
            );
        }
    }

    #[test]
    fn should_accept_url_of_maximum_length_and_reject_longer() {
        let prefix = "https://example.com/";
        let longest = format!(
            "{prefix}{}",
            "a".repeat(AdditionalUrl::MAX_CHARS - prefix.len())
        );
        assert_eq!(longest, url(&longest).as_str());

        let too_long = format!("{longest}a");
        assert_eq!(
            Err(AdditionalUrlError::TooLong {
                max_chars: 2048,
                actual_chars: 2049,
            }),
            AdditionalUrl::try_from(too_long.as_str())
        );
    }

    #[test]
    fn should_reject_url_that_exceeds_maximum_length_after_normalization() {
        let prefix = "https://example.com/";
        let value = format!(
            "{prefix}{}é",
            "a".repeat(AdditionalUrl::MAX_CHARS - prefix.len() - 1)
        );

        assert!(matches!(
            AdditionalUrl::try_from(value.as_str()),
            Err(AdditionalUrlError::TooLong { .. })
        ));
    }

    #[test]
    fn should_not_reveal_url_in_debug_output() {
        assert!(!format!("{:?}", url("https://secret.example/")).contains("secret"));
    }

    #[test]
    fn should_accept_zero_to_ten_urls() {
        assert!(AdditionalUrls::try_from(Vec::new()).unwrap().is_empty());
        let ten = (0..10)
            .map(|index| url(&format!("https://example.com/{index}")))
            .collect::<Vec<_>>();
        assert_eq!(10, AdditionalUrls::try_from(ten).unwrap().len());
    }

    #[test]
    fn should_reject_more_than_ten_urls() {
        let eleven = (0..11)
            .map(|index| url(&format!("https://example.com/{index}")))
            .collect::<Vec<_>>();

        assert_eq!(
            Err(AdditionalUrlsError::TooMany {
                max_items: 10,
                actual_items: 11,
            }),
            AdditionalUrls::try_from(eleven)
        );
    }

    #[test]
    fn should_reject_urls_that_are_equal_after_normalization() {
        let urls = vec![
            url("https://example.com/a"),
            url("https://example.com/b"),
            url("HTTPS://EXAMPLE.com:443/a"),
        ];

        assert_eq!(
            Err(AdditionalUrlsError::Duplicate { index: 2 }),
            AdditionalUrls::try_from(urls)
        );
    }
}
