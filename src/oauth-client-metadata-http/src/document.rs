use oauth_core::metadata_document_client::ClientMetadataDocumentClaims;
use serde::Deserialize;

/// Members a metadata document must not carry: a public client has no secret.
const SECRET_MEMBERS: [&str; 2] = ["client_secret", "client_secret_expires_at"];

/// The wire members Aura reads. Unknown members are ignored, as RFC 7591 requires.
#[derive(Deserialize)]
struct MetadataDocumentWire {
    client_id: Option<String>,
    client_name: Option<String>,
    redirect_uris: Option<Vec<String>>,
    token_endpoint_auth_method: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MalformedDocument {
    #[error("metadata document is not a JSON object")]
    NotObject(#[source] serde_json::Error),
    #[error("metadata document member has an unexpected type")]
    MemberType(#[source] serde_json::Error),
}

pub(crate) fn parse_claims(body: &[u8]) -> Result<ClientMetadataDocumentClaims, MalformedDocument> {
    let members = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(body)
        .map_err(MalformedDocument::NotObject)?;
    let declares_client_secret = SECRET_MEMBERS
        .iter()
        .any(|member| members.contains_key(*member));
    let wire = serde_json::from_value::<MetadataDocumentWire>(serde_json::Value::Object(members))
        .map_err(MalformedDocument::MemberType)?;
    Ok(ClientMetadataDocumentClaims {
        client_id: wire.client_id,
        client_name: wire.client_name,
        redirect_uris: wire.redirect_uris,
        token_endpoint_auth_method: wire.token_endpoint_auth_method,
        grant_types: wire.grant_types,
        response_types: wire.response_types,
        client_uri: wire.client_uri,
        logo_uri: wire.logo_uri,
        declares_client_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_the_members_aura_reads_and_ignore_others() {
        let claims = parse_claims(
            br#"{
                "client_id": "https://agent.example/client.json",
                "client_name": "Agent",
                "redirect_uris": ["http://127.0.0.1/callback"],
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "client_uri": "https://agent.example",
                "logo_uri": "https://agent.example/logo.png",
                "scope": "users:read",
                "software_id": "agent"
            }"#,
        )
        .unwrap();

        assert_eq!(
            ClientMetadataDocumentClaims {
                client_id: Some("https://agent.example/client.json".to_owned()),
                client_name: Some("Agent".to_owned()),
                redirect_uris: Some(vec!["http://127.0.0.1/callback".to_owned()]),
                token_endpoint_auth_method: Some("none".to_owned()),
                grant_types: Some(vec![
                    "authorization_code".to_owned(),
                    "refresh_token".to_owned()
                ]),
                response_types: Some(vec!["code".to_owned()]),
                client_uri: Some("https://agent.example".to_owned()),
                logo_uri: Some("https://agent.example/logo.png".to_owned()),
                declares_client_secret: false,
            },
            claims
        );
        assert_eq!(
            ClientMetadataDocumentClaims::default(),
            parse_claims(b"{}").unwrap()
        );
    }

    #[test]
    fn should_report_declared_secrets_even_when_empty() {
        for body in [
            &br#"{"client_secret": "s3cr3t"}"#[..],
            br#"{"client_secret": null}"#,
            br#"{"client_secret_expires_at": 0}"#,
        ] {
            assert!(parse_claims(body).unwrap().declares_client_secret);
        }
    }

    #[test]
    fn should_reject_documents_that_are_not_metadata_objects() {
        for body in [
            &b""[..],
            b"[]",
            b"\"https://agent.example/client.json\"",
            b"{\"client_id\": ",
            b"<html></html>",
        ] {
            assert!(matches!(
                parse_claims(body),
                Err(MalformedDocument::NotObject(_))
            ));
        }
        for body in [
            &br#"{"redirect_uris": "http://127.0.0.1/callback"}"#[..],
            br#"{"client_name": 7}"#,
            br#"{"grant_types": [1]}"#,
        ] {
            assert!(matches!(
                parse_claims(body),
                Err(MalformedDocument::MemberType(_))
            ));
        }
    }
}
