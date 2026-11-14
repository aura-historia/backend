use domain_primitives::versioned::Versioned;
use sqlx::FromRow;
use std::collections::HashSet;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken,
    InvalidMetadataDocumentClientId, MetadataDocumentClientId, OAuthConnectionId,
    RehydratedAccessTokenState, Scope,
};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenAuthentication, AccessTokenDetails, AccessTokenStorageVersion, VersionedAccessToken,
};

pub(crate) const ACCESS_TOKEN_COLUMNS: &str = "access_token_id, user_id, token_short, token_hash, name, scopes, origin, oauth_client_id, oauth_metadata_document_client_id, oauth_connection_id, expires_at, version";

#[derive(Debug, FromRow)]
pub(crate) struct AccessTokenRow {
    pub access_token_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub token_short: String,
    pub token_hash: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub origin: String,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub oauth_metadata_document_client_id: Option<String>,
    pub oauth_connection_id: Option<uuid::Uuid>,
    pub expires_at: Option<time::OffsetDateTime>,
    pub version: i64,
}

#[derive(Debug, FromRow)]
pub(crate) struct AccessTokenDetailsRow {
    pub access_token_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub origin: String,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub oauth_metadata_document_client_id: Option<String>,
    pub oauth_connection_id: Option<uuid::Uuid>,
    pub expires_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, FromRow)]
pub(crate) struct AccessTokenAuthenticationRow {
    pub access_token_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub scopes: Vec<String>,
    pub origin: String,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub oauth_metadata_document_client_id: Option<String>,
    pub oauth_connection_id: Option<uuid::Uuid>,
    pub expires_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccessTokenRowMappingError {
    #[error("invalid access token scope: {0}")]
    InvalidScope(String),
    #[error("invalid access token origin: {0}")]
    InvalidOrigin(String),
    #[error("OAuth access token must record exactly the client and connection of its origin")]
    MissingOAuthClientId,
    #[error("user access token has unexpected OAuth client id")]
    UnexpectedOAuthClientId,
    #[error("invalid persisted access token identifier")]
    InvalidAccessTokenId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid persisted user identifier")]
    InvalidUserId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid persisted OAuth client identifier")]
    InvalidOAuthClientId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid persisted metadata document client identifier")]
    InvalidMetadataDocumentClientId(#[source] InvalidMetadataDocumentClientId),
    #[error("invalid persisted OAuth connection identifier")]
    InvalidOAuthConnectionId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid access token version")]
    InvalidVersion(#[from] domain_primitives::version::InvalidVersionError),
}

impl TryFrom<AccessTokenRow> for VersionedAccessToken {
    type Error = AccessTokenRowMappingError;

    fn try_from(row: AccessTokenRow) -> Result<Self, Self::Error> {
        let version = AccessTokenStorageVersion::try_from(row.version)?;
        let access_token = access_token_from_parts(AccessTokenPersistedState {
            access_token_id: row.access_token_id,
            user_id: row.user_id,
            token_short: row.token_short,
            token_hash: row.token_hash,
            name: row.name,
            scopes: row.scopes,
            origin: row.origin,
            oauth_client_id: row.oauth_client_id,
            oauth_metadata_document_client_id: row.oauth_metadata_document_client_id,
            oauth_connection_id: row.oauth_connection_id,
            expires_at: row.expires_at,
        })?;

        Ok(Versioned::new(access_token, version))
    }
}

impl TryFrom<AccessTokenDetailsRow> for AccessTokenDetails {
    type Error = AccessTokenRowMappingError;

    fn try_from(row: AccessTokenDetailsRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::try_from(row.user_id)
                .map_err(AccessTokenRowMappingError::InvalidUserId)?,
            access_token_id: AccessTokenId::try_from(row.access_token_id)
                .map_err(AccessTokenRowMappingError::InvalidAccessTokenId)?,
            name: AccessTokenName::from(row.name),
            scopes: parse_scopes(row.scopes)?,
            origin: parse_origin(PersistedOrigin {
                origin: row.origin,
                oauth_client_id: row.oauth_client_id,
                oauth_metadata_document_client_id: row.oauth_metadata_document_client_id,
                oauth_connection_id: row.oauth_connection_id,
            })?,
            expires: row.expires_at,
        })
    }
}

impl TryFrom<AccessTokenAuthenticationRow> for AccessTokenAuthentication {
    type Error = AccessTokenRowMappingError;

    fn try_from(row: AccessTokenAuthenticationRow) -> Result<Self, Self::Error> {
        Ok(Self {
            access_token_id: AccessTokenId::try_from(row.access_token_id)
                .map_err(AccessTokenRowMappingError::InvalidAccessTokenId)?,
            user_id: UserId::try_from(row.user_id)
                .map_err(AccessTokenRowMappingError::InvalidUserId)?,
            scopes: parse_scopes(row.scopes)?,
            origin: parse_origin(PersistedOrigin {
                origin: row.origin,
                oauth_client_id: row.oauth_client_id,
                oauth_metadata_document_client_id: row.oauth_metadata_document_client_id,
                oauth_connection_id: row.oauth_connection_id,
            })?,
            expires: row.expires_at,
        })
    }
}

pub(crate) fn scope_values(scopes: &HashSet<Scope>) -> Vec<&'static str> {
    scopes.iter().copied().map(Scope::as_str).collect()
}

/// The persisted origin columns of an access token.
pub(crate) struct OriginValues<'a> {
    pub origin: &'static str,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub oauth_metadata_document_client_id: Option<&'a str>,
    pub oauth_connection_id: Option<uuid::Uuid>,
}

/// The origin value and the client columns that match it.
pub(crate) fn access_token_origin_values(access_token: &AccessToken) -> OriginValues<'_> {
    match access_token.origin() {
        AccessTokenOrigin::User => OriginValues {
            origin: "USER",
            oauth_client_id: None,
            oauth_metadata_document_client_id: None,
            oauth_connection_id: None,
        },
        AccessTokenOrigin::OAuth { client_id } => OriginValues {
            origin: "OAUTH",
            oauth_client_id: Some(client_id.into_uuid()),
            oauth_metadata_document_client_id: None,
            oauth_connection_id: None,
        },
        AccessTokenOrigin::OAuthMetadataDocument {
            client_id,
            connection_id,
        } => OriginValues {
            origin: "OAUTH_METADATA_DOCUMENT",
            oauth_client_id: None,
            oauth_metadata_document_client_id: Some(client_id.as_str()),
            oauth_connection_id: Some(connection_id.into_uuid()),
        },
    }
}

struct AccessTokenPersistedState {
    access_token_id: uuid::Uuid,
    user_id: uuid::Uuid,
    token_short: String,
    token_hash: String,
    name: String,
    scopes: Vec<String>,
    origin: String,
    oauth_client_id: Option<uuid::Uuid>,
    oauth_metadata_document_client_id: Option<String>,
    oauth_connection_id: Option<uuid::Uuid>,
    expires_at: Option<time::OffsetDateTime>,
}

fn access_token_from_parts(
    state: AccessTokenPersistedState,
) -> Result<AccessToken, AccessTokenRowMappingError> {
    Ok(AccessToken::rehydrate(RehydratedAccessTokenState {
        id: AccessTokenId::try_from(state.access_token_id)
            .map_err(AccessTokenRowMappingError::InvalidAccessTokenId)?,
        hashed_token: HashedRawAccessToken::new(state.token_short, state.token_hash),
        user_id: UserId::try_from(state.user_id)
            .map_err(AccessTokenRowMappingError::InvalidUserId)?,
        name: AccessTokenName::from(state.name),
        scopes: parse_scopes(state.scopes)?,
        origin: parse_origin(PersistedOrigin {
            origin: state.origin,
            oauth_client_id: state.oauth_client_id,
            oauth_metadata_document_client_id: state.oauth_metadata_document_client_id,
            oauth_connection_id: state.oauth_connection_id,
        })?,
        expires: state.expires_at,
    }))
}

pub(crate) fn parse_scopes(
    values: Vec<String>,
) -> Result<HashSet<Scope>, AccessTokenRowMappingError> {
    values
        .into_iter()
        .map(|value| match value.as_str() {
            "auctions:read" => Ok(Scope::AuctionsRead),
            "product-listings:read" => Ok(Scope::ProductListingsRead),
            "auctions:write" => Ok(Scope::AuctionsWrite),
            "listing-sources:read" => Ok(Scope::ListingSourcesRead),
            "parties:read" => Ok(Scope::PartiesRead),
            "parties:write" => Ok(Scope::PartiesWrite),
            "partnership-applications:read" => Ok(Scope::PartnershipApplicationsRead),
            "partnership-applications:write" => Ok(Scope::PartnershipApplicationsWrite),
            "partnerships:read" => Ok(Scope::PartnershipsRead),
            "partnerships:write" => Ok(Scope::PartnershipsWrite),
            "admin-overview:read" => Ok(Scope::AdminOverviewRead),
            "content-notices:read" => Ok(Scope::ContentNoticesRead),
            "search-filters:read" => Ok(Scope::SearchFiltersRead),
            "notifications:read" => Ok(Scope::NotificationsRead),
            "notifications:write" => Ok(Scope::NotificationsWrite),
            "product-listings:write" => Ok(Scope::ProductListingsWrite),
            "listing-sources:write" => Ok(Scope::ListingSourcesWrite),
            "users:read" => Ok(Scope::UsersRead),
            "users:write" => Ok(Scope::UsersWrite),
            "access-tokens:read" => Ok(Scope::AccessTokensRead),
            "access-tokens:write" => Ok(Scope::AccessTokensWrite),
            "search-filters:write" => Ok(Scope::SearchFiltersWrite),
            "watchlist:read" => Ok(Scope::WatchlistRead),
            "watchlist:write" => Ok(Scope::WatchlistWrite),
            _ => Err(AccessTokenRowMappingError::InvalidScope(value)),
        })
        .collect()
}

pub(crate) struct PersistedOrigin {
    pub origin: String,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub oauth_metadata_document_client_id: Option<String>,
    pub oauth_connection_id: Option<uuid::Uuid>,
}

pub(crate) fn parse_origin(
    persisted: PersistedOrigin,
) -> Result<AccessTokenOrigin, AccessTokenRowMappingError> {
    match (
        persisted.origin.as_str(),
        persisted.oauth_client_id,
        persisted.oauth_metadata_document_client_id,
        persisted.oauth_connection_id,
    ) {
        ("USER", None, None, None) => Ok(AccessTokenOrigin::User),
        ("USER", _, _, _) => Err(AccessTokenRowMappingError::UnexpectedOAuthClientId),
        ("OAUTH", Some(client_id), None, None) => client_id
            .try_into()
            .map(|client_id| AccessTokenOrigin::OAuth { client_id })
            .map_err(AccessTokenRowMappingError::InvalidOAuthClientId),
        ("OAUTH_METADATA_DOCUMENT", None, Some(client_id), Some(connection_id)) => {
            Ok(AccessTokenOrigin::OAuthMetadataDocument {
                client_id: MetadataDocumentClientId::try_from(client_id)
                    .map_err(AccessTokenRowMappingError::InvalidMetadataDocumentClientId)?,
                connection_id: OAuthConnectionId::try_from(connection_id)
                    .map_err(AccessTokenRowMappingError::InvalidOAuthConnectionId)?,
            })
        }
        ("OAUTH" | "OAUTH_METADATA_DOCUMENT", _, _, _) => {
            Err(AccessTokenRowMappingError::MissingOAuthClientId)
        }
        _ => Err(AccessTokenRowMappingError::InvalidOrigin(persisted.origin)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[derive(Clone, Copy, Debug)]
    enum PersistedObjectIdCase {
        AccessToken,
        User,
        OAuthClient,
    }

    const PERSISTED_OBJECT_ID_CASES: [PersistedObjectIdCase; 3] = [
        PersistedObjectIdCase::AccessToken,
        PersistedObjectIdCase::User,
        PersistedObjectIdCase::OAuthClient,
    ];

    fn uuid_v7_fixture() -> Uuid {
        Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f75)
    }

    fn uuid_v4_fixture() -> Uuid {
        Uuid::from_u128(0x550e8400e29b41d4a716446655440000)
    }

    #[test]
    fn should_map_listing_source_write_scope_from_storage() {
        assert_eq!(
            HashSet::from([Scope::ListingSourcesWrite]),
            parse_scopes(vec!["listing-sources:write".to_owned()])
                .unwrap_or_else(|_| unreachable!())
        );
    }

    fn persisted_origin(
        origin: &str,
        oauth_client_id: Option<Uuid>,
        oauth_metadata_document_client_id: Option<&str>,
        oauth_connection_id: Option<Uuid>,
    ) -> PersistedOrigin {
        PersistedOrigin {
            origin: origin.to_owned(),
            oauth_client_id,
            oauth_metadata_document_client_id: oauth_metadata_document_client_id.map(str::to_owned),
            oauth_connection_id,
        }
    }

    #[test]
    fn should_require_exactly_the_client_columns_of_each_origin() {
        let client_id = "https://agent.example/client.json";
        let registered = Some(uuid_v7_fixture());
        let connection = OAuthConnectionId::new();
        let document = Some(client_id);

        assert_eq!(
            AccessTokenOrigin::OAuthMetadataDocument {
                client_id: client_id.parse().unwrap(),
                connection_id: connection,
            },
            parse_origin(persisted_origin(
                "OAUTH_METADATA_DOCUMENT",
                None,
                document,
                Some(connection.into_uuid())
            ))
            .unwrap()
        );
        assert_eq!(
            AccessTokenOrigin::User,
            parse_origin(persisted_origin("USER", None, None, None)).unwrap()
        );
        let connection = Some(connection.into_uuid());
        for (origin, oauth_client_id, metadata_document_client_id, connection_id) in [
            ("USER", None, document, None),
            ("USER", None, None, connection),
            ("OAUTH", registered, document, None),
            ("OAUTH", registered, None, connection),
            ("OAUTH", None, document, connection),
            ("OAUTH_METADATA_DOCUMENT", registered, document, connection),
            ("OAUTH_METADATA_DOCUMENT", None, document, None),
            ("OAUTH_METADATA_DOCUMENT", registered, None, None),
            ("OAUTH_METADATA_DOCUMENT", None, None, connection),
        ] {
            assert!(
                matches!(
                    parse_origin(persisted_origin(
                        origin,
                        oauth_client_id,
                        metadata_document_client_id,
                        connection_id
                    )),
                    Err(AccessTokenRowMappingError::MissingOAuthClientId
                        | AccessTokenRowMappingError::UnexpectedOAuthClientId)
                ),
                "{origin} {oauth_client_id:?} {metadata_document_client_id:?} {connection_id:?}"
            );
        }
        assert!(matches!(
            parse_origin(persisted_origin(
                "OAUTH_METADATA_DOCUMENT",
                None,
                Some("https://agent.example:443/client.json"),
                connection
            )),
            Err(AccessTokenRowMappingError::InvalidMetadataDocumentClientId(
                _
            ))
        ));
        assert!(matches!(
            parse_origin(persisted_origin(
                "OAUTH_METADATA_DOCUMENT",
                None,
                document,
                Some(uuid_v4_fixture())
            )),
            Err(AccessTokenRowMappingError::InvalidOAuthConnectionId(_))
        ));
    }

    #[test]
    fn should_round_trip_canonical_auction_read_scope() {
        let scopes = HashSet::from([Scope::AuctionsRead]);
        assert_eq!(vec!["auctions:read"], scope_values(&scopes));
        assert_eq!(
            scopes,
            parse_scopes(vec!["auctions:read".to_owned()]).expect("canonical scope")
        );
        for invalid in ["auction:read", "AUCTIONS:READ", "auctions:delete"] {
            assert!(matches!(
                parse_scopes(vec![invalid.to_owned()]),
                Err(AccessTokenRowMappingError::InvalidScope(_))
            ));
        }
    }

    #[test]
    fn should_round_trip_canonical_product_listing_read_scope() {
        let scopes = HashSet::from([Scope::ProductListingsRead]);
        assert_eq!(vec!["product-listings:read"], scope_values(&scopes));
        assert_eq!(
            scopes,
            parse_scopes(vec!["product-listings:read".to_owned()]).expect("canonical scope")
        );
        for invalid in [
            "product-listing:read",
            "PRODUCT-LISTINGS:READ",
            "product-listings:delete",
        ] {
            assert!(matches!(
                parse_scopes(vec![invalid.to_owned()]),
                Err(AccessTokenRowMappingError::InvalidScope(_))
            ));
        }
    }

    fn persisted_ids(case: PersistedObjectIdCase) -> (Uuid, Uuid, Uuid) {
        let valid = uuid_v7_fixture();
        let invalid = uuid_v4_fixture();

        match case {
            PersistedObjectIdCase::AccessToken => (invalid, valid, valid),
            PersistedObjectIdCase::User => (valid, invalid, valid),
            PersistedObjectIdCase::OAuthClient => (valid, valid, invalid),
        }
    }

    fn expected_error(error: &AccessTokenRowMappingError, case: PersistedObjectIdCase) -> bool {
        match case {
            PersistedObjectIdCase::AccessToken => {
                matches!(error, AccessTokenRowMappingError::InvalidAccessTokenId(_))
            }
            PersistedObjectIdCase::User => {
                matches!(error, AccessTokenRowMappingError::InvalidUserId(_))
            }
            PersistedObjectIdCase::OAuthClient => {
                matches!(error, AccessTokenRowMappingError::InvalidOAuthClientId(_))
            }
        }
    }

    #[test]
    fn should_reject_uuid_v4_object_ids_when_mapping_access_token_rows() {
        for case in PERSISTED_OBJECT_ID_CASES {
            let (access_token_id, user_id, oauth_client_id) = persisted_ids(case);
            let row = AccessTokenRow {
                access_token_id,
                user_id,
                token_short: "short".to_owned(),
                token_hash: "hash".to_owned(),
                name: "Token".to_owned(),
                scopes: vec![],
                origin: "OAUTH".to_owned(),
                oauth_client_id: Some(oauth_client_id),
                oauth_metadata_document_client_id: None,
                oauth_connection_id: None,
                expires_at: None,
                version: 1,
            };

            let result = VersionedAccessToken::try_from(row);
            assert!(
                matches!(result, Err(ref error) if expected_error(error, case)),
                "access-token row accepted UUIDv4 for {case:?}"
            );
        }
    }

    #[test]
    fn should_reject_uuid_v4_object_ids_when_mapping_access_token_details_rows() {
        for case in PERSISTED_OBJECT_ID_CASES {
            let (access_token_id, user_id, oauth_client_id) = persisted_ids(case);
            let row = AccessTokenDetailsRow {
                access_token_id,
                user_id,
                name: "Token".to_owned(),
                scopes: vec![],
                origin: "OAUTH".to_owned(),
                oauth_client_id: Some(oauth_client_id),
                oauth_metadata_document_client_id: None,
                oauth_connection_id: None,
                expires_at: None,
            };

            let result = AccessTokenDetails::try_from(row);
            assert!(
                matches!(result, Err(ref error) if expected_error(error, case)),
                "access-token details row accepted UUIDv4 for {case:?}"
            );
        }
    }

    #[test]
    fn should_reject_uuid_v4_object_ids_when_mapping_access_token_authentication_rows() {
        for case in PERSISTED_OBJECT_ID_CASES {
            let (access_token_id, user_id, oauth_client_id) = persisted_ids(case);
            let row = AccessTokenAuthenticationRow {
                access_token_id,
                user_id,
                scopes: vec![],
                origin: "OAUTH".to_owned(),
                oauth_client_id: Some(oauth_client_id),
                oauth_metadata_document_client_id: None,
                oauth_connection_id: None,
                expires_at: None,
            };

            let result = AccessTokenAuthentication::try_from(row);
            assert!(
                matches!(result, Err(ref error) if expected_error(error, case)),
                "access-token authentication row accepted UUIDv4 for {case:?}"
            );
        }
    }
    #[test]
    fn should_round_trip_new_credential_scopes() {
        for (scope, value) in [
            (Scope::AuctionsWrite, "auctions:write"),
            (Scope::ListingSourcesRead, "listing-sources:read"),
            (Scope::PartiesRead, "parties:read"),
            (Scope::PartiesWrite, "parties:write"),
            (
                Scope::PartnershipApplicationsRead,
                "partnership-applications:read",
            ),
            (
                Scope::PartnershipApplicationsWrite,
                "partnership-applications:write",
            ),
            (Scope::PartnershipsRead, "partnerships:read"),
            (Scope::PartnershipsWrite, "partnerships:write"),
            (Scope::AdminOverviewRead, "admin-overview:read"),
            (Scope::ContentNoticesRead, "content-notices:read"),
            (Scope::SearchFiltersRead, "search-filters:read"),
            (Scope::NotificationsRead, "notifications:read"),
            (Scope::NotificationsWrite, "notifications:write"),
        ] {
            assert_eq!(
                HashSet::from([scope]),
                parse_scopes(vec![value.to_owned()]).expect("canonical scope")
            );
            assert_eq!(value, scope.as_str());
        }
    }
}
