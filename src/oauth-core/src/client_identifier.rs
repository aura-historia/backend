use credential_core::metadata_document_client_id::{
    InvalidMetadataDocumentClientId, MetadataDocumentClientId,
};
use credential_core::oauth_client_id::OAuthClientId;
use std::fmt;

/// The two kinds of OAuth client Aura accepts, told apart by the submitted `client_id`.
///
/// An administrator registers a confidential client and Aura stores it under an `oc_` ID. A
/// public client identifies itself by the HTTPS URL of its Client ID Metadata Document, which
/// Aura fetches instead of storing a registration. The kinds never fall back to each other.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OAuthClientIdentifier {
    Registered(OAuthClientId),
    MetadataDocument(MetadataDocumentClientId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidOAuthClientIdentifier {
    #[error("client_id is not a registered OAuth client ID")]
    Registered,
    #[error("client_id is not a valid metadata document URL")]
    MetadataDocument(#[source] InvalidMetadataDocumentClientId),
}

impl OAuthClientIdentifier {
    /// Classifies a submitted `client_id`. Any value that parses as an `https` URL is a
    /// metadata document client and must be one exactly; every other value must be a
    /// registered client ID.
    pub fn parse(value: &str) -> Result<Self, InvalidOAuthClientIdentifier> {
        if url::Url::parse(value).is_ok_and(|url| url.scheme() == "https") {
            MetadataDocumentClientId::try_from(value)
                .map(Self::MetadataDocument)
                .map_err(InvalidOAuthClientIdentifier::MetadataDocument)
        } else {
            value
                .parse::<OAuthClientId>()
                .map(Self::Registered)
                .map_err(|_| InvalidOAuthClientIdentifier::Registered)
        }
    }
}

impl From<OAuthClientId> for OAuthClientIdentifier {
    fn from(value: OAuthClientId) -> Self {
        Self::Registered(value)
    }
}

impl From<MetadataDocumentClientId> for OAuthClientIdentifier {
    fn from(value: MetadataDocumentClientId) -> Self {
        Self::MetadataDocument(value)
    }
}

impl fmt::Display for OAuthClientIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registered(client_id) => client_id.fmt(formatter),
            Self::MetadataDocument(client_id) => client_id.fmt(formatter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_registered_and_metadata_document_client_ids() {
        let registered = OAuthClientId::new();

        assert_eq!(
            Ok(OAuthClientIdentifier::Registered(registered)),
            OAuthClientIdentifier::parse(&registered.to_string())
        );
        let url = "https://client.example/client.json";
        let parsed = OAuthClientIdentifier::parse(url).unwrap();
        assert_eq!(
            OAuthClientIdentifier::MetadataDocument(
                MetadataDocumentClientId::try_from(url).unwrap()
            ),
            parsed
        );
        assert_eq!(url, parsed.to_string());
        assert_eq!(
            registered.to_string(),
            OAuthClientIdentifier::from(registered).to_string()
        );
    }

    #[test]
    fn should_never_fall_back_between_client_kinds() {
        for value in [
            "https://client.example",
            "https://client.example:8443/client.json",
            "https://Client.Example/client.json",
            "HTTPS://client.example/client.json",
            "https://user@client.example/client.json",
        ] {
            assert!(
                matches!(
                    OAuthClientIdentifier::parse(value),
                    Err(InvalidOAuthClientIdentifier::MetadataDocument(_))
                ),
                "{value}"
            );
        }
        for value in [
            "",
            "oc_not-a-typeid",
            "01890a5d-ac96-774b-bf1d-d5586c639f75",
            "http://client.example/client.json",
            "client.example/client.json",
            "usr_01h455vb4pex5vy7enb1p677vn",
        ] {
            assert_eq!(
                Err(InvalidOAuthClientIdentifier::Registered),
                OAuthClientIdentifier::parse(value),
                "{value}"
            );
        }
    }
}
