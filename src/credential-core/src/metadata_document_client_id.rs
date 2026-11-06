use std::fmt;
use std::str::FromStr;
use url::{Host, Url};

/// Upper bound for a metadata-document client identifier, so it stays a reasonable column and
/// log value.
pub const MAX_METADATA_DOCUMENT_CLIENT_ID_LENGTH: usize = 2048;

/// The `client_id` of an OAuth client that publishes its metadata at this exact HTTPS URL
/// (draft-ietf-oauth-client-id-metadata-document).
///
/// Only the URL's canonical serialization is accepted, so the identifier compares by exact
/// string equality and is the URL that is fetched. It names a host, uses the default HTTPS
/// port, has a path below the root and carries no credentials or fragment.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MetadataDocumentClientId(Url);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidMetadataDocumentClientId {
    #[error("metadata document client ID is not an absolute URL")]
    NotUrl,
    #[error("metadata document client ID must use HTTPS")]
    NotHttps,
    #[error("metadata document client ID must be in canonical URL form")]
    NotCanonical,
    #[error("metadata document client ID must name a domain host")]
    NotDomainHost,
    #[error("metadata document client ID must use the default HTTPS port")]
    NonDefaultPort,
    #[error("metadata document client ID must not carry credentials")]
    Credentials,
    #[error("metadata document client ID must not carry a fragment")]
    Fragment,
    #[error("metadata document client ID must have a path below the root")]
    MissingPath,
    #[error("metadata document client ID exceeds {MAX_METADATA_DOCUMENT_CLIENT_ID_LENGTH} bytes")]
    TooLong,
}

impl MetadataDocumentClientId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The document URL, identical to the identifier.
    pub fn url(&self) -> &Url {
        &self.0
    }

    /// The lowercase domain that serves the document.
    pub fn host(&self) -> &str {
        self.0
            .host_str()
            .expect("validated metadata document client IDs name a host")
    }
}

impl TryFrom<&str> for MetadataDocumentClientId {
    type Error = InvalidMetadataDocumentClientId;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > MAX_METADATA_DOCUMENT_CLIENT_ID_LENGTH {
            return Err(InvalidMetadataDocumentClientId::TooLong);
        }
        let url = Url::parse(value).map_err(|_| InvalidMetadataDocumentClientId::NotUrl)?;
        if url.scheme() != "https" {
            return Err(InvalidMetadataDocumentClientId::NotHttps);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(InvalidMetadataDocumentClientId::Credentials);
        }
        if url.fragment().is_some() {
            return Err(InvalidMetadataDocumentClientId::Fragment);
        }
        if !matches!(url.host(), Some(Host::Domain(_))) {
            return Err(InvalidMetadataDocumentClientId::NotDomainHost);
        }
        if url.port().is_some() {
            return Err(InvalidMetadataDocumentClientId::NonDefaultPort);
        }
        if url.path() == "/" {
            return Err(InvalidMetadataDocumentClientId::MissingPath);
        }
        // Normalization would make distinct submitted strings name one client: an explicit
        // `:443`, dot segments, letter case and percent-encoding variants are rejected instead.
        if url.as_str() != value {
            return Err(InvalidMetadataDocumentClientId::NotCanonical);
        }
        Ok(Self(url))
    }
}

impl TryFrom<String> for MetadataDocumentClientId {
    type Error = InvalidMetadataDocumentClientId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for MetadataDocumentClientId {
    type Err = InvalidMetadataDocumentClientId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl fmt::Display for MetadataDocumentClientId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for MetadataDocumentClientId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MetadataDocumentClientId")
            .field(&self.as_str())
            .finish()
    }
}

impl From<MetadataDocumentClientId> for String {
    fn from(value: MetadataDocumentClientId) -> Self {
        value.0.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_canonical_https_document_urls() {
        for value in [
            "https://claude.ai/oauth/claude-code-client-metadata",
            "https://client.example/.well-known/client.json",
            "https://client.example/client.json?version=2",
        ] {
            let client_id = MetadataDocumentClientId::try_from(value).unwrap();

            assert_eq!(value, client_id.as_str());
            assert_eq!(value, client_id.to_string());
        }
        assert_eq!(
            "claude.ai",
            MetadataDocumentClientId::try_from("https://claude.ai/client")
                .unwrap()
                .host()
        );
    }

    #[test]
    fn should_reject_values_that_are_not_canonical_document_urls() {
        for (value, expected) in [
            (
                "oc_01h455vb4pex5vy7enb1p677vn",
                InvalidMetadataDocumentClientId::NotUrl,
            ),
            (
                "client.example/client.json",
                InvalidMetadataDocumentClientId::NotUrl,
            ),
            (
                "http://client.example/client.json",
                InvalidMetadataDocumentClientId::NotHttps,
            ),
            (
                "https://client.example",
                InvalidMetadataDocumentClientId::MissingPath,
            ),
            (
                "https://client.example/",
                InvalidMetadataDocumentClientId::MissingPath,
            ),
            (
                "https://client.example:443/client.json",
                InvalidMetadataDocumentClientId::NotCanonical,
            ),
            (
                "https://client.example:8443/client.json",
                InvalidMetadataDocumentClientId::NonDefaultPort,
            ),
            (
                "https://Client.Example/client.json",
                InvalidMetadataDocumentClientId::NotCanonical,
            ),
            (
                "https://client.example/a/../client.json",
                InvalidMetadataDocumentClientId::NotCanonical,
            ),
            (
                "https://client.example/./client.json",
                InvalidMetadataDocumentClientId::NotCanonical,
            ),
            (
                "https://client.example/client.json#x",
                InvalidMetadataDocumentClientId::Fragment,
            ),
            (
                "https://user@client.example/client.json",
                InvalidMetadataDocumentClientId::Credentials,
            ),
            (
                "https://203.0.113.10/client.json",
                InvalidMetadataDocumentClientId::NotDomainHost,
            ),
            (
                "https://[2001:db8::1]/client.json",
                InvalidMetadataDocumentClientId::NotDomainHost,
            ),
            (
                " https://client.example/client.json",
                InvalidMetadataDocumentClientId::NotCanonical,
            ),
        ] {
            assert_eq!(
                Err(expected),
                MetadataDocumentClientId::try_from(value),
                "{value}"
            );
        }
        let long = format!(
            "https://client.example/{}",
            "a".repeat(MAX_METADATA_DOCUMENT_CLIENT_ID_LENGTH)
        );
        assert_eq!(
            Err(InvalidMetadataDocumentClientId::TooLong),
            MetadataDocumentClientId::try_from(long.as_str())
        );
    }
}
