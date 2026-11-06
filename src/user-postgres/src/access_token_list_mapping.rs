use crate::access_token_mapping::{AccessTokenDetailsRow, AccessTokenRowMappingError};
use sqlx::FromRow;
use time::OffsetDateTime;
use user_core::access_token::{MetadataDocumentClientId, OAuthConnectionId};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenDetails, AccessTokenListEntry, InvalidAccessTokenListEntry, ListedAccessToken,
    ListedOAuthConnection, RegisteredOAuthClientDisplay,
};

/// A user's listing entries: individually managed tokens with their registered client, and
/// metadata document client connections, which stand for their rotating tokens. `$1` binds
/// the user. Registered client URIs are stored in canonical URL form, so the host is the
/// authority after any user information.
pub(crate) const ACCESS_TOKEN_LIST_ENTRIES: &str = "\
    SELECT 'ACCESS_TOKEN' AS entry_kind, tokens.access_token_id AS entry_id, tokens.user_id, \
           tokens.name, tokens.scopes, tokens.origin, tokens.oauth_client_id, \
           tokens.oauth_metadata_document_client_id, tokens.oauth_connection_id, \
           tokens.expires_at, tokens.created, NULL::timestamptz AS last_used, \
           clients.name AS client_name, \
           substring(clients.client_uri FROM '^[a-z][a-z0-9+.-]*://(?:[^/?#@]*@)?(\\[[^]/?#]*\\]|[^/?#:]+)') \
               AS client_host \
    FROM access_tokens AS tokens \
    LEFT JOIN oauth_clients AS clients ON clients.client_id = tokens.oauth_client_id \
    WHERE tokens.user_id = $1 AND tokens.oauth_connection_id IS NULL \
    UNION ALL \
    SELECT 'CONNECTION', connections.connection_id, connections.user_id, \
           connections.client_name, connections.scopes, 'OAUTH_METADATA_DOCUMENT', NULL, \
           connections.client_id, connections.connection_id, NULL, connections.created, \
           connections.last_used, NULL, NULL \
    FROM oauth_connections AS connections \
    WHERE connections.user_id = $1";

#[derive(Debug, FromRow)]
pub(crate) struct AccessTokenListEntryRow {
    pub entry_kind: String,
    pub entry_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub origin: String,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub oauth_metadata_document_client_id: Option<String>,
    pub oauth_connection_id: Option<uuid::Uuid>,
    pub expires_at: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub last_used: Option<OffsetDateTime>,
    pub client_name: Option<String>,
    pub client_host: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccessTokenListEntryMappingError {
    #[error("invalid listed access token")]
    AccessToken(#[from] AccessTokenRowMappingError),
    #[error("inconsistent listed access token")]
    InvalidEntry(#[from] InvalidAccessTokenListEntry),
    #[error("listed registered client must have a name and a client URI host")]
    IncompleteClient,
    #[error("listed connection is incomplete")]
    IncompleteConnection,
    #[error("invalid listed connection identifier")]
    InvalidConnectionId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid listed user identifier")]
    InvalidUserId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid listed metadata document client identifier")]
    InvalidMetadataDocumentClientId(
        #[source] user_core::access_token::InvalidMetadataDocumentClientId,
    ),
    #[error("invalid listing entry kind: {0}")]
    InvalidKind(String),
}

impl AccessTokenListEntryRow {
    pub(crate) fn created(&self) -> OffsetDateTime {
        self.created
    }
}

impl TryFrom<AccessTokenListEntryRow> for AccessTokenListEntry {
    type Error = AccessTokenListEntryMappingError;

    fn try_from(row: AccessTokenListEntryRow) -> Result<Self, Self::Error> {
        match row.entry_kind.as_str() {
            "ACCESS_TOKEN" => listed_access_token(row).map(Self::AccessToken),
            "CONNECTION" => listed_connection(row).map(Self::Connection),
            _ => Err(AccessTokenListEntryMappingError::InvalidKind(
                row.entry_kind,
            )),
        }
    }
}

fn listed_access_token(
    row: AccessTokenListEntryRow,
) -> Result<ListedAccessToken, AccessTokenListEntryMappingError> {
    let client = match (row.client_name, row.client_host) {
        (None, None) => None,
        (Some(name), Some(host)) if !host.is_empty() => {
            Some(RegisteredOAuthClientDisplay { name, host })
        }
        _ => return Err(AccessTokenListEntryMappingError::IncompleteClient),
    };
    let token = AccessTokenDetails::try_from(AccessTokenDetailsRow {
        access_token_id: row.entry_id,
        user_id: row.user_id,
        name: row.name,
        scopes: row.scopes,
        origin: row.origin,
        oauth_client_id: row.oauth_client_id,
        oauth_metadata_document_client_id: row.oauth_metadata_document_client_id,
        oauth_connection_id: row.oauth_connection_id,
        expires_at: row.expires_at,
    })?;
    Ok(ListedAccessToken::new(token, client)?)
}

fn listed_connection(
    row: AccessTokenListEntryRow,
) -> Result<ListedOAuthConnection, AccessTokenListEntryMappingError> {
    let (Some(client_id), Some(last_used)) = (row.oauth_metadata_document_client_id, row.last_used)
    else {
        return Err(AccessTokenListEntryMappingError::IncompleteConnection);
    };
    Ok(ListedOAuthConnection {
        connection_id: OAuthConnectionId::try_from(row.entry_id)
            .map_err(AccessTokenListEntryMappingError::InvalidConnectionId)?,
        user_id: UserId::try_from(row.user_id)
            .map_err(AccessTokenListEntryMappingError::InvalidUserId)?,
        client_id: MetadataDocumentClientId::try_from(client_id)
            .map_err(AccessTokenListEntryMappingError::InvalidMetadataDocumentClientId)?,
        client_name: row.name,
        scopes: crate::access_token_mapping::parse_scopes(row.scopes)?,
        created: row.created,
        last_used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use user_core::access_token::{AccessTokenOrigin, OAuthClientId, Scope};

    fn token_row(origin: &str) -> AccessTokenListEntryRow {
        AccessTokenListEntryRow {
            entry_kind: "ACCESS_TOKEN".to_owned(),
            entry_id: user_core::access_token::AccessTokenId::new().into_uuid(),
            user_id: UserId::new().into_uuid(),
            name: "token".to_owned(),
            scopes: vec!["users:read".to_owned()],
            origin: origin.to_owned(),
            oauth_client_id: None,
            oauth_metadata_document_client_id: None,
            oauth_connection_id: None,
            expires_at: None,
            created: OffsetDateTime::now_utc(),
            last_used: None,
            client_name: None,
            client_host: None,
        }
    }

    fn connection_row() -> AccessTokenListEntryRow {
        let connection_id = OAuthConnectionId::new().into_uuid();
        AccessTokenListEntryRow {
            entry_kind: "CONNECTION".to_owned(),
            entry_id: connection_id,
            origin: "OAUTH_METADATA_DOCUMENT".to_owned(),
            oauth_metadata_document_client_id: Some("https://agent.example/client.json".to_owned()),
            oauth_connection_id: Some(connection_id),
            last_used: Some(OffsetDateTime::now_utc()),
            name: "Agent".to_owned(),
            ..token_row("OAUTH_METADATA_DOCUMENT")
        }
    }

    #[test]
    fn should_map_registered_client_tokens_with_their_client() {
        let client_id = OAuthClientId::new();
        let entry = AccessTokenListEntry::try_from(AccessTokenListEntryRow {
            oauth_client_id: Some(client_id.into_uuid()),
            client_name: Some("Shop".to_owned()),
            client_host: Some("shop.example".to_owned()),
            ..token_row("OAUTH")
        })
        .unwrap();

        let AccessTokenListEntry::AccessToken(listed) = entry else {
            panic!("expected a token entry");
        };
        assert_eq!(AccessTokenOrigin::OAuth { client_id }, listed.token.origin);
        assert_eq!(
            Some(RegisteredOAuthClientDisplay {
                name: "Shop".to_owned(),
                host: "shop.example".to_owned()
            }),
            listed.client
        );
        assert!(matches!(
            AccessTokenListEntry::try_from(AccessTokenListEntryRow {
                oauth_client_id: Some(client_id.into_uuid()),
                client_name: Some("Shop".to_owned()),
                ..token_row("OAUTH")
            }),
            Err(AccessTokenListEntryMappingError::IncompleteClient)
        ));
    }

    #[test]
    fn should_map_connections_and_reject_incomplete_ones() {
        let AccessTokenListEntry::Connection(connection) =
            AccessTokenListEntry::try_from(connection_row()).unwrap()
        else {
            panic!("expected a connection entry");
        };
        assert_eq!("agent.example", connection.client_id.host());
        assert_eq!("Agent", connection.client_name);
        assert_eq!(HashSet::from([Scope::UsersRead]), connection.scopes);

        assert!(matches!(
            AccessTokenListEntry::try_from(AccessTokenListEntryRow {
                last_used: None,
                ..connection_row()
            }),
            Err(AccessTokenListEntryMappingError::IncompleteConnection)
        ));
        assert!(matches!(
            AccessTokenListEntry::try_from(AccessTokenListEntryRow {
                entry_kind: "OTHER".to_owned(),
                ..connection_row()
            }),
            Err(AccessTokenListEntryMappingError::InvalidKind(_))
        ));
    }
}
