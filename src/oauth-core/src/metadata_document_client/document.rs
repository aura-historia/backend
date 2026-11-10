use super::redirect_uri::{InvalidMetadataDocumentRedirectUri, MetadataDocumentRedirectUri};
use crate::client::OAuthClientName;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use url::Url;

/// Upper bound for a self-asserted client name shown on the consent page.
pub const MAX_METADATA_DOCUMENT_CLIENT_NAME_CHARS: usize = 128;

/// The metadata members Aura reads from a fetched client metadata document, before
/// validation. The fetching adapter maps its wire format to this shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientMetadataDocumentClaims {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub redirect_uris: Option<Vec<String>>,
    pub token_endpoint_auth_method: Option<String>,
    /// Methods the client says it can use, a member some clients publish beside
    /// `token_endpoint_auth_method`.
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    /// Whether the document declares a client secret, which a metadata document must not.
    pub declares_client_secret: bool,
}

/// A validated client metadata document of a public client.
///
/// The client authenticates with its `client_id` alone and uses the authorization code grant;
/// only the listed callbacks may receive its authorization responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientMetadataDocument {
    client_id: MetadataDocumentClientId,
    name: OAuthClientName,
    redirect_uris: Vec<MetadataDocumentRedirectUri>,
    client_uri: Option<Url>,
    logo_uri: Option<Url>,
    declares_refresh_token_grant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidClientMetadataDocument {
    #[error("document client_id does not equal the document URL")]
    ClientIdMismatch,
    #[error("document client_name is missing or invalid")]
    ClientName,
    #[error("document redirect_uris are missing")]
    MissingRedirectUris,
    #[error("document redirect URI is invalid")]
    RedirectUri(#[source] InvalidMetadataDocumentRedirectUri),
    #[error("document must be able to authenticate as a public client with method none")]
    TokenEndpointAuthMethod,
    #[error("document must not declare a client secret")]
    ClientSecret,
    #[error("document grant_types must contain authorization_code")]
    GrantTypes,
    #[error("document response_types must contain code")]
    ResponseTypes,
    #[error("document {0} must be an HTTPS URL")]
    MetadataUri(&'static str),
}

impl ClientMetadataDocument {
    /// Validates the document fetched from `client_id`.
    pub fn validate(
        client_id: &MetadataDocumentClientId,
        claims: ClientMetadataDocumentClaims,
    ) -> Result<Self, InvalidClientMetadataDocument> {
        if claims.client_id.as_deref() != Some(client_id.as_str()) {
            return Err(InvalidClientMetadataDocument::ClientIdMismatch);
        }
        if claims.declares_client_secret {
            return Err(InvalidClientMetadataDocument::ClientSecret);
        }
        if !authenticates_as_public_client(&claims) {
            return Err(InvalidClientMetadataDocument::TokenEndpointAuthMethod);
        }
        if !contains(claims.grant_types.as_deref(), "authorization_code") {
            return Err(InvalidClientMetadataDocument::GrantTypes);
        }
        if claims
            .response_types
            .as_deref()
            .is_some_and(|types| !contains(Some(types), "code"))
        {
            return Err(InvalidClientMetadataDocument::ResponseTypes);
        }
        Ok(Self {
            client_id: client_id.clone(),
            name: client_name(claims.client_name.as_deref())?,
            redirect_uris: redirect_uris(claims.redirect_uris)?,
            client_uri: https_metadata_uri(claims.client_uri.as_deref(), "client_uri")?,
            logo_uri: https_metadata_uri(claims.logo_uri.as_deref(), "logo_uri")?,
            declares_refresh_token_grant: contains(claims.grant_types.as_deref(), "refresh_token"),
        })
    }

    pub fn client_id(&self) -> &MetadataDocumentClientId {
        &self.client_id
    }

    pub fn name(&self) -> &OAuthClientName {
        &self.name
    }

    pub fn client_uri(&self) -> Option<&Url> {
        self.client_uri.as_ref()
    }

    pub fn logo_uri(&self) -> Option<&Url> {
        self.logo_uri.as_ref()
    }

    /// Whether `requested` is a callback this document lists.
    pub fn admits_redirect_uri(&self, requested: &Url) -> bool {
        self.redirect_uris
            .iter()
            .any(|redirect_uri| redirect_uri.admits(requested))
    }

    /// Whether the document's `grant_types` lists `refresh_token`, which asks for refresh
    /// tokens without the `offline_access` scope.
    pub fn declares_refresh_token_grant(&self) -> bool {
        self.declares_refresh_token_grant
    }

    /// Whether every listed callback returns to the user's own device.
    pub fn lists_only_loopback_redirect_uris(&self) -> bool {
        self.redirect_uris
            .iter()
            .all(MetadataDocumentRedirectUri::is_loopback)
    }
}

fn contains(values: Option<&[String]>, expected: &str) -> bool {
    values.is_some_and(|values| values.iter().any(|value| value == expected))
}

/// Aura never verifies client assertions, so a document must declare method `none`. One
/// that declares `private_key_jwt` is accepted only when it also lists `none` as a method it
/// can use, because the token endpoint then authenticates the client by `client_id` alone,
/// protected by PKCE like every other metadata document client, and ignores any assertion.
fn authenticates_as_public_client(claims: &ClientMetadataDocumentClaims) -> bool {
    match claims.token_endpoint_auth_method.as_deref() {
        Some("none") => true,
        Some("private_key_jwt") => contains(
            claims.token_endpoint_auth_methods_supported.as_deref(),
            "none",
        ),
        _ => false,
    }
}

fn client_name(value: Option<&str>) -> Result<OAuthClientName, InvalidClientMetadataDocument> {
    let name = value.map(str::trim).unwrap_or_default();
    if name.is_empty()
        || name.chars().count() > MAX_METADATA_DOCUMENT_CLIENT_NAME_CHARS
        || name
            .chars()
            .any(|c| c.is_control() || is_invisible_formatting(c))
    {
        return Err(InvalidClientMetadataDocument::ClientName);
    }
    Ok(OAuthClientName::from(name))
}

/// Zero-width and bidirectional formatting characters could make a self-asserted name
/// render as another client's on the consent page.
fn is_invisible_formatting(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn redirect_uris(
    values: Option<Vec<String>>,
) -> Result<Vec<MetadataDocumentRedirectUri>, InvalidClientMetadataDocument> {
    let values = values
        .filter(|values| !values.is_empty())
        .ok_or(InvalidClientMetadataDocument::MissingRedirectUris)?;
    values
        .iter()
        .map(|value| {
            MetadataDocumentRedirectUri::try_from(value.as_str())
                .map_err(InvalidClientMetadataDocument::RedirectUri)
        })
        .collect()
}

fn https_metadata_uri(
    value: Option<&str>,
    member: &'static str,
) -> Result<Option<Url>, InvalidClientMetadataDocument> {
    value
        .map(|value| {
            Url::parse(value)
                .ok()
                .filter(|url| url.scheme() == "https" && url.host().is_some())
                .ok_or(InvalidClientMetadataDocument::MetadataUri(member))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT_ID: &str = "https://client.example/oauth/client.json";

    fn client_id() -> MetadataDocumentClientId {
        MetadataDocumentClientId::try_from(CLIENT_ID).unwrap()
    }

    fn claims() -> ClientMetadataDocumentClaims {
        ClientMetadataDocumentClaims {
            client_id: Some(CLIENT_ID.to_owned()),
            client_name: Some("Example Agent".to_owned()),
            redirect_uris: Some(vec![
                "http://localhost/callback".to_owned(),
                "http://127.0.0.1/callback".to_owned(),
            ]),
            token_endpoint_auth_method: Some("none".to_owned()),
            token_endpoint_auth_methods_supported: None,
            grant_types: Some(vec![
                "authorization_code".to_owned(),
                "refresh_token".to_owned(),
            ]),
            response_types: Some(vec!["code".to_owned()]),
            client_uri: Some("https://client.example".to_owned()),
            logo_uri: Some("https://client.example/logo.png".to_owned()),
            declares_client_secret: false,
        }
    }

    fn validate(
        claims: ClientMetadataDocumentClaims,
    ) -> Result<ClientMetadataDocument, InvalidClientMetadataDocument> {
        ClientMetadataDocument::validate(&client_id(), claims)
    }

    #[test]
    fn should_accept_a_public_authorization_code_client_document() {
        let document = validate(claims()).unwrap();

        assert_eq!(&client_id(), document.client_id());
        assert_eq!("Example Agent", document.name().as_ref());
        assert_eq!(
            Some("https://client.example/"),
            document.client_uri().map(Url::as_str)
        );
        assert_eq!(
            Some("https://client.example/logo.png"),
            document.logo_uri().map(Url::as_str)
        );
        assert!(document.lists_only_loopback_redirect_uris());
        assert!(
            document.admits_redirect_uri(&Url::parse("http://localhost:4123/callback").unwrap())
        );
        assert!(
            !document.admits_redirect_uri(&Url::parse("https://client.example/callback").unwrap())
        );
    }

    #[test]
    fn should_record_whether_the_document_declares_the_refresh_token_grant() {
        assert!(validate(claims()).unwrap().declares_refresh_token_grant());
        assert!(
            !validate(ClientMetadataDocumentClaims {
                grant_types: Some(vec!["authorization_code".to_owned()]),
                ..claims()
            })
            .unwrap()
            .declares_refresh_token_grant()
        );
    }

    #[test]
    fn should_accept_optional_members_when_absent() {
        let document = validate(ClientMetadataDocumentClaims {
            redirect_uris: Some(vec!["https://client.example/callback".to_owned()]),
            response_types: None,
            client_uri: None,
            logo_uri: None,
            ..claims()
        })
        .unwrap();

        assert_eq!(None, document.client_uri());
        assert_eq!(None, document.logo_uri());
        assert!(!document.lists_only_loopback_redirect_uris());
    }

    #[test]
    fn should_reject_a_document_whose_client_id_is_not_the_document_url() {
        for client_id in [
            None,
            Some("https://client.example/oauth/other.json"),
            Some("https://client.example:443/oauth/client.json"),
            Some("https://CLIENT.example/oauth/client.json"),
            Some("https://client.example/oauth/client.json/"),
        ] {
            assert_eq!(
                Err(InvalidClientMetadataDocument::ClientIdMismatch),
                validate(ClientMetadataDocumentClaims {
                    client_id: client_id.map(str::to_owned),
                    ..claims()
                }),
                "{client_id:?}"
            );
        }
    }

    #[test]
    fn should_reject_documents_that_are_not_public_authorization_code_clients() {
        for (claims, expected) in [
            (
                ClientMetadataDocumentClaims {
                    token_endpoint_auth_method: None,
                    ..claims()
                },
                InvalidClientMetadataDocument::TokenEndpointAuthMethod,
            ),
            (
                ClientMetadataDocumentClaims {
                    token_endpoint_auth_method: Some("client_secret_post".to_owned()),
                    ..claims()
                },
                InvalidClientMetadataDocument::TokenEndpointAuthMethod,
            ),
            (
                ClientMetadataDocumentClaims {
                    token_endpoint_auth_method: Some("private_key_jwt".to_owned()),
                    ..claims()
                },
                InvalidClientMetadataDocument::TokenEndpointAuthMethod,
            ),
            (
                ClientMetadataDocumentClaims {
                    token_endpoint_auth_method: Some("private_key_jwt".to_owned()),
                    token_endpoint_auth_methods_supported: Some(vec!["private_key_jwt".to_owned()]),
                    ..claims()
                },
                InvalidClientMetadataDocument::TokenEndpointAuthMethod,
            ),
            (
                ClientMetadataDocumentClaims {
                    token_endpoint_auth_method: Some("client_secret_post".to_owned()),
                    token_endpoint_auth_methods_supported: Some(vec!["none".to_owned()]),
                    ..claims()
                },
                InvalidClientMetadataDocument::TokenEndpointAuthMethod,
            ),
            (
                ClientMetadataDocumentClaims {
                    token_endpoint_auth_method: None,
                    token_endpoint_auth_methods_supported: Some(vec!["none".to_owned()]),
                    ..claims()
                },
                InvalidClientMetadataDocument::TokenEndpointAuthMethod,
            ),
            (
                ClientMetadataDocumentClaims {
                    declares_client_secret: true,
                    ..claims()
                },
                InvalidClientMetadataDocument::ClientSecret,
            ),
            (
                ClientMetadataDocumentClaims {
                    grant_types: None,
                    ..claims()
                },
                InvalidClientMetadataDocument::GrantTypes,
            ),
            (
                ClientMetadataDocumentClaims {
                    grant_types: Some(vec!["client_credentials".to_owned()]),
                    ..claims()
                },
                InvalidClientMetadataDocument::GrantTypes,
            ),
            (
                ClientMetadataDocumentClaims {
                    response_types: Some(vec!["token".to_owned()]),
                    ..claims()
                },
                InvalidClientMetadataDocument::ResponseTypes,
            ),
        ] {
            assert_eq!(Err(expected), validate(claims));
        }
    }

    #[test]
    fn should_accept_a_private_key_jwt_document_that_also_lists_none() {
        let document = validate(ClientMetadataDocumentClaims {
            token_endpoint_auth_method: Some("private_key_jwt".to_owned()),
            token_endpoint_auth_methods_supported: Some(vec![
                "none".to_owned(),
                "private_key_jwt".to_owned(),
            ]),
            ..claims()
        });

        assert!(document.is_ok(), "{document:?}");
    }

    #[test]
    fn should_require_a_displayable_client_name() {
        for name in [
            None,
            Some(String::new()),
            Some("   ".to_owned()),
            Some("Agent\u{7}".to_owned()),
            Some("Agent\u{202E}tnegA".to_owned()),
            Some("Ag\u{200B}ent".to_owned()),
            Some("a".repeat(MAX_METADATA_DOCUMENT_CLIENT_NAME_CHARS + 1)),
        ] {
            assert_eq!(
                Err(InvalidClientMetadataDocument::ClientName),
                validate(ClientMetadataDocumentClaims {
                    client_name: name.clone(),
                    ..claims()
                }),
                "{name:?}"
            );
        }
        assert_eq!(
            "Agent",
            validate(ClientMetadataDocumentClaims {
                client_name: Some("  Agent ".to_owned()),
                ..claims()
            })
            .unwrap()
            .name()
            .as_ref()
        );
    }

    #[test]
    fn should_require_valid_redirect_uris() {
        assert_eq!(
            Err(InvalidClientMetadataDocument::MissingRedirectUris),
            validate(ClientMetadataDocumentClaims {
                redirect_uris: None,
                ..claims()
            })
        );
        assert_eq!(
            Err(InvalidClientMetadataDocument::MissingRedirectUris),
            validate(ClientMetadataDocumentClaims {
                redirect_uris: Some(Vec::new()),
                ..claims()
            })
        );
        assert_eq!(
            Err(InvalidClientMetadataDocument::RedirectUri(
                InvalidMetadataDocumentRedirectUri::UnsupportedScheme
            )),
            validate(ClientMetadataDocumentClaims {
                redirect_uris: Some(vec![
                    "https://client.example/callback".to_owned(),
                    "http://client.example/callback".to_owned(),
                ]),
                ..claims()
            })
        );
    }

    #[test]
    fn should_require_https_client_and_logo_uris_when_present() {
        for (claims, member) in [
            (
                ClientMetadataDocumentClaims {
                    client_uri: Some("http://client.example".to_owned()),
                    ..claims()
                },
                "client_uri",
            ),
            (
                ClientMetadataDocumentClaims {
                    logo_uri: Some("data:image/png;base64,AAAA".to_owned()),
                    ..claims()
                },
                "logo_uri",
            ),
            (
                ClientMetadataDocumentClaims {
                    logo_uri: Some("logo.png".to_owned()),
                    ..claims()
                },
                "logo_uri",
            ),
        ] {
            assert_eq!(
                Err(InvalidClientMetadataDocument::MetadataUri(member)),
                validate(claims)
            );
        }
    }
}
