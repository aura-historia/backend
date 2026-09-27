use reqwest::header::HeaderValue;
use url::{Host, Url};

#[derive(Clone, PartialEq, Eq)]
pub struct LoopsNewsletterConfig {
    pub(crate) newsletter_list_id: String,
    pub(crate) update_url: Url,
    pub(crate) authorization: HeaderValue,
}

#[derive(Debug, thiserror::Error)]
pub enum LoopsNewsletterConfigError {
    #[error("missing Loops API key")]
    MissingApiKey,
    #[error("invalid Loops API key header")]
    InvalidApiKey,
    #[error("missing or invalid Loops newsletter list ID")]
    InvalidListId,
    #[error("invalid Loops API base URL")]
    InvalidBaseUrl,
}

impl LoopsNewsletterConfig {
    pub fn new(
        api_key: String,
        newsletter_list_id: String,
        api_base_url: String,
    ) -> Result<Self, LoopsNewsletterConfigError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(LoopsNewsletterConfigError::MissingApiKey);
        }

        let newsletter_list_id = newsletter_list_id.trim().to_owned();
        if newsletter_list_id.is_empty() || newsletter_list_id.chars().any(char::is_whitespace) {
            return Err(LoopsNewsletterConfigError::InvalidListId);
        }

        let base = Url::parse(api_base_url.trim())
            .map_err(|_| LoopsNewsletterConfigError::InvalidBaseUrl)?;
        let loopback = match base.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        let allowed_scheme = base.scheme() == "https" || (base.scheme() == "http" && loopback);
        if !allowed_scheme
            || base.host().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(LoopsNewsletterConfigError::InvalidBaseUrl);
        }

        let update_url = Url::parse(&format!(
            "{}/v1/contacts/update",
            base.as_str().trim_end_matches('/')
        ))
        .map_err(|_| LoopsNewsletterConfigError::InvalidBaseUrl)?;

        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| LoopsNewsletterConfigError::InvalidApiKey)?;
        authorization.set_sensitive(true);

        Ok(Self {
            newsletter_list_id,
            update_url,
            authorization,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LoopsNewsletterConfig, LoopsNewsletterConfigError};

    const API_KEY: &str = "test-api-key";
    const LIST_ID: &str = "newsletter-list-id";

    fn config(api_key: &str, list_id: &str, base_url: &str) -> LoopsNewsletterConfig {
        LoopsNewsletterConfig::new(api_key.into(), list_id.into(), base_url.into())
            .unwrap_or_else(|error| panic!("invalid test Loops config: {error}"))
    }

    #[test]
    fn rejects_missing_or_blank_credentials_and_list_ids() {
        for api_key in ["", "   "] {
            assert!(matches!(
                LoopsNewsletterConfig::new(
                    api_key.into(),
                    LIST_ID.into(),
                    "https://app.loops.so/api".into()
                ),
                Err(LoopsNewsletterConfigError::MissingApiKey)
            ));
        }
        for list_id in ["", "   ", "newsletter list"] {
            assert!(matches!(
                LoopsNewsletterConfig::new(
                    API_KEY.into(),
                    list_id.into(),
                    "https://app.loops.so/api".into()
                ),
                Err(LoopsNewsletterConfigError::InvalidListId)
            ));
        }
    }

    #[test]
    fn accepts_https_and_loopback_http_and_normalizes_trailing_slash() {
        let default = config(API_KEY, LIST_ID, "https://app.loops.so/api");
        let trailing_slash = config(API_KEY, LIST_ID, "https://app.loops.so/api/");
        assert!(default == trailing_slash);

        let loopback = config(API_KEY, LIST_ID, "http://127.0.0.1:3000/api");
        assert_eq!(
            "http://127.0.0.1:3000/api/v1/contacts/update",
            loopback.update_url.as_str()
        );
        assert_eq!(
            "https://app.loops.so/api/v1/contacts/update",
            default.update_url.as_str()
        );
    }

    #[test]
    fn rejects_unsafe_or_malformed_base_urls() {
        for base_url in [
            "not a URL",
            "http://example.com/api",
            "ftp://app.loops.so/api",
            "https://user:password@app.loops.so/api",
            "https://app.loops.so/api?token=secret",
            "https://app.loops.so/api#fragment",
        ] {
            assert!(
                matches!(
                    LoopsNewsletterConfig::new(API_KEY.into(), LIST_ID.into(), base_url.into()),
                    Err(LoopsNewsletterConfigError::InvalidBaseUrl)
                ),
                "unexpectedly accepted {base_url}"
            );
        }
    }

    #[test]
    fn rejects_api_keys_that_cannot_be_used_as_authorization_headers() {
        assert!(matches!(
            LoopsNewsletterConfig::new(
                "invalid\nkey".into(),
                LIST_ID.into(),
                "https://app.loops.so/api".into()
            ),
            Err(LoopsNewsletterConfigError::InvalidApiKey)
        ));
    }

    #[test]
    fn configuration_errors_do_not_retain_the_supplied_secret() {
        let sentinel = "loops-secret-sentinel";
        let error = match LoopsNewsletterConfig::new(
            sentinel.into(),
            "invalid list id".into(),
            "https://app.loops.so/api".into(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid list unexpectedly accepted"),
        };

        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }
}
