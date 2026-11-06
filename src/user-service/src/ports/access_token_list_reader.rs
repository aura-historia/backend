use crate::ports::access_token_details_reader::AccessTokenDetails;
use application::error::BoxError;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessTokenId, AccessTokenOrigin, MetadataDocumentClientId, OAuthConnectionId, Scope,
};
use user_core::user_id::UserId;

/// One entry of a user's credential listing: a credential the user manages individually, or
/// a connection standing for all the rotating credentials of one metadata document client.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessTokenListEntry {
    AccessToken(ListedAccessToken),
    Connection(ListedOAuthConnection),
}

/// A personal access token, or one issued to a registered OAuth client.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedAccessToken {
    pub token: AccessTokenDetails,
    /// The registered client holding the token; present exactly for OAuth tokens.
    pub client: Option<RegisteredOAuthClientDisplay>,
}

/// How the listing names an administrator-registered OAuth client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredOAuthClientDisplay {
    pub name: String,
    /// The host of the client's registered `client_uri`.
    pub host: String,
}

/// A user's connection to a public client identified by its metadata document.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedOAuthConnection {
    pub connection_id: OAuthConnectionId,
    pub user_id: UserId,
    pub client_id: MetadataDocumentClientId,
    /// The name the client's document asserted at the user's last approval.
    pub client_name: String,
    /// The union of the scopes the user approved for the client.
    pub scopes: HashSet<Scope>,
    pub created: OffsetDateTime,
    pub last_used: OffsetDateTime,
}

/// Identifies a listing entry; entries are ordered by creation time, then by this ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTokenListEntryId {
    AccessToken(AccessTokenId),
    Connection(OAuthConnectionId),
}

impl AccessTokenListEntry {
    pub fn id(&self) -> AccessTokenListEntryId {
        match self {
            Self::AccessToken(listed) => {
                AccessTokenListEntryId::AccessToken(listed.token.access_token_id)
            }
            Self::Connection(connection) => {
                AccessTokenListEntryId::Connection(connection.connection_id)
            }
        }
    }

    pub fn user_id(&self) -> UserId {
        match self {
            Self::AccessToken(listed) => listed.token.user_id,
            Self::Connection(connection) => connection.user_id,
        }
    }
}

/// Why a persisted listing entry is inconsistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAccessTokenListEntry {
    #[error("a listed OAuth token must name its registered client, and only OAuth tokens may")]
    ClientMismatch,
    #[error("metadata document client tokens are listed through their connection")]
    ConnectionToken,
}

impl ListedAccessToken {
    /// Pairs a token with its client, rejecting combinations the listing cannot show.
    pub fn new(
        token: AccessTokenDetails,
        client: Option<RegisteredOAuthClientDisplay>,
    ) -> Result<Self, InvalidAccessTokenListEntry> {
        match (&token.origin, &client) {
            (AccessTokenOrigin::User, None) | (AccessTokenOrigin::OAuth { .. }, Some(_)) => {
                Ok(Self { token, client })
            }
            (AccessTokenOrigin::OAuthMetadataDocument { .. }, _) => {
                Err(InvalidAccessTokenListEntry::ConnectionToken)
            }
            _ => Err(InvalidAccessTokenListEntry::ClientMismatch),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenListReadError {
    #[error("temporary access token list read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid access token list read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal access token list read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AccessTokenListReader: Send + Sync {
    /// Every credential entry of the user, oldest first.
    async fn list_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AccessTokenListEntry>, AccessTokenListReadError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use user_core::access_token::{AccessTokenName, OAuthClientId};

    fn details(origin: AccessTokenOrigin) -> AccessTokenDetails {
        AccessTokenDetails {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            name: AccessTokenName::from("token"),
            scopes: HashSet::new(),
            origin,
            expires: None,
        }
    }

    #[test]
    fn should_name_the_client_of_exactly_the_registered_client_tokens() {
        let client = RegisteredOAuthClientDisplay {
            name: "Shop".to_owned(),
            host: "shop.example".to_owned(),
        };
        let oauth = AccessTokenOrigin::OAuth {
            client_id: OAuthClientId::new(),
        };

        assert!(ListedAccessToken::new(details(AccessTokenOrigin::User), None).is_ok());
        assert!(ListedAccessToken::new(details(oauth.clone()), Some(client.clone())).is_ok());
        assert_eq!(
            Err(InvalidAccessTokenListEntry::ClientMismatch),
            ListedAccessToken::new(details(oauth), None)
        );
        assert_eq!(
            Err(InvalidAccessTokenListEntry::ClientMismatch),
            ListedAccessToken::new(details(AccessTokenOrigin::User), Some(client))
        );
        assert_eq!(
            Err(InvalidAccessTokenListEntry::ConnectionToken),
            ListedAccessToken::new(
                details(AccessTokenOrigin::OAuthMetadataDocument {
                    client_id: "https://agent.example/client.json".parse().unwrap(),
                    connection_id: OAuthConnectionId::new(),
                }),
                None
            )
        );
    }
}
