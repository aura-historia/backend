use std::net::{Ipv4Addr, Ipv6Addr};
use url::{Host, Url};

/// A callback listed in a client metadata document.
///
/// Callbacks use HTTPS, except native-app loopback callbacks, which use plain HTTP on
/// `127.0.0.1`, `[::1]` or `localhost`. A loopback callback matches a request on any port
/// (RFC 8252 §7.3), because native apps bind an ephemeral port; every other part must match
/// exactly. Every other callback matches only exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataDocumentRedirectUri(Url);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidMetadataDocumentRedirectUri {
    #[error("redirect URI is not an absolute URL")]
    NotUrl,
    #[error("redirect URI must use HTTPS or loopback HTTP")]
    UnsupportedScheme,
    #[error("redirect URI must not carry credentials")]
    Credentials,
    #[error("redirect URI must not carry a fragment")]
    Fragment,
}

impl TryFrom<&str> for MetadataDocumentRedirectUri {
    type Error = InvalidMetadataDocumentRedirectUri;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let url = Url::parse(value).map_err(|_| InvalidMetadataDocumentRedirectUri::NotUrl)?;
        let supported = match url.scheme() {
            "https" => url.host().is_some(),
            "http" => is_loopback(&url),
            _ => false,
        };
        if !supported {
            return Err(InvalidMetadataDocumentRedirectUri::UnsupportedScheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(InvalidMetadataDocumentRedirectUri::Credentials);
        }
        if url.fragment().is_some() {
            return Err(InvalidMetadataDocumentRedirectUri::Fragment);
        }
        Ok(Self(url))
    }
}

impl MetadataDocumentRedirectUri {
    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn is_loopback(&self) -> bool {
        is_loopback(&self.0)
    }

    /// Whether `requested` may receive the authorization response for this listed callback.
    pub fn admits(&self, requested: &Url) -> bool {
        if !self.is_loopback() {
            return &self.0 == requested;
        }
        requested.scheme() == "http"
            && requested.host() == self.0.host()
            && requested.username().is_empty()
            && requested.password().is_none()
            && requested.path() == self.0.path()
            && requested.query() == self.0.query()
            && requested.fragment().is_none()
    }
}

fn is_loopback(url: &Url) -> bool {
    url.scheme() == "http"
        && match url.host() {
            Some(Host::Ipv4(address)) => address == Ipv4Addr::LOCALHOST,
            Some(Host::Ipv6(address)) => address == Ipv6Addr::LOCALHOST,
            Some(Host::Domain(domain)) => domain == "localhost",
            None => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(value: &str) -> MetadataDocumentRedirectUri {
        MetadataDocumentRedirectUri::try_from(value).unwrap()
    }

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn should_match_loopback_callbacks_on_any_port() {
        for (listed_uri, requested) in [
            (
                "http://127.0.0.1/callback",
                "http://127.0.0.1:54321/callback",
            ),
            (
                "http://127.0.0.1:8080/callback",
                "http://127.0.0.1/callback",
            ),
            ("http://[::1]/callback", "http://[::1]:3000/callback"),
            (
                "http://localhost/callback",
                "http://localhost:33418/callback",
            ),
            (
                "http://localhost/callback?mode=cli",
                "http://localhost:1/callback?mode=cli",
            ),
        ] {
            assert!(listed(listed_uri).admits(&url(requested)), "{requested}");
        }
    }

    #[test]
    fn should_match_every_other_part_of_a_loopback_callback_exactly() {
        let listed_uri = listed("http://127.0.0.1/callback");
        for requested in [
            "http://localhost:54321/callback",
            "http://[::1]:54321/callback",
            "http://127.0.0.2:54321/callback",
            "https://127.0.0.1:54321/callback",
            "http://127.0.0.1:54321/callback/other",
            "http://127.0.0.1:54321/",
            "http://127.0.0.1:54321/callback?x=1",
            "http://127.0.0.1:54321/callback#fragment",
            "http://user@127.0.0.1:54321/callback",
        ] {
            assert!(!listed_uri.admits(&url(requested)), "{requested}");
        }
    }

    #[test]
    fn should_match_https_callbacks_exactly_including_the_port() {
        let listed_uri = listed("https://client.example/callback");

        assert!(listed_uri.admits(&url("https://client.example/callback")));
        for requested in [
            "https://client.example:8443/callback",
            "https://client.example/callback/",
            "https://client.example/callback?x=1",
            "http://client.example/callback",
            "https://other.example/callback",
        ] {
            assert!(!listed_uri.admits(&url(requested)), "{requested}");
        }
        assert!(!listed("https://localhost/callback").is_loopback());
        assert!(!listed("https://localhost/callback").admits(&url("https://localhost:1/callback")));
    }

    #[test]
    fn should_reject_callbacks_that_are_neither_https_nor_loopback_http() {
        for (value, expected) in [
            (
                "http://client.example/callback",
                InvalidMetadataDocumentRedirectUri::UnsupportedScheme,
            ),
            (
                "http://127.0.0.2/callback",
                InvalidMetadataDocumentRedirectUri::UnsupportedScheme,
            ),
            (
                "http://localhost.example/callback",
                InvalidMetadataDocumentRedirectUri::UnsupportedScheme,
            ),
            (
                "cursor://anysphere.cursor/callback",
                InvalidMetadataDocumentRedirectUri::UnsupportedScheme,
            ),
            (
                "javascript:alert(1)",
                InvalidMetadataDocumentRedirectUri::UnsupportedScheme,
            ),
            ("/callback", InvalidMetadataDocumentRedirectUri::NotUrl),
            (
                "https://client.example/callback#x",
                InvalidMetadataDocumentRedirectUri::Fragment,
            ),
            (
                "https://user:pass@client.example/callback",
                InvalidMetadataDocumentRedirectUri::Credentials,
            ),
        ] {
            assert_eq!(
                Err(expected),
                MetadataDocumentRedirectUri::try_from(value),
                "{value}"
            );
        }
    }
}
