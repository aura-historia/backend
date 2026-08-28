use domain_primitives::versioned::Versioned;
use sqlx::FromRow;
use std::collections::HashSet;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken,
    RehydratedAccessTokenState, Scope,
};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenAuthentication, AccessTokenDetails, AccessTokenStorageVersion, VersionedAccessToken,
};

pub(crate) const ACCESS_TOKEN_COLUMNS: &str = "access_token_id, user_id, token_short, token_hash, name, scopes, origin, oauth_client_id, expires_at, version";

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
    pub expires_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, FromRow)]
pub(crate) struct AccessTokenAuthenticationRow {
    pub access_token_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub scopes: Vec<String>,
    pub origin: String,
    pub oauth_client_id: Option<uuid::Uuid>,
    pub expires_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccessTokenRowMappingError {
    #[error("invalid access token scope: {0}")]
    InvalidScope(String),
    #[error("invalid access token origin: {0}")]
    InvalidOrigin(String),
    #[error("OAuth access token missing OAuth client id")]
    MissingOAuthClientId,
    #[error("user access token has unexpected OAuth client id")]
    UnexpectedOAuthClientId,
    #[error("invalid access token identifier")]
    InvalidIdentifier(#[source] uuid::Error),
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
            expires_at: row.expires_at,
        })?;

        Ok(Versioned::new(access_token, version))
    }
}

impl TryFrom<AccessTokenDetailsRow> for AccessTokenDetails {
    type Error = AccessTokenRowMappingError;

    fn try_from(row: AccessTokenDetailsRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::from(row.user_id),
            access_token_id: AccessTokenId::from(row.access_token_id),
            name: AccessTokenName::from(row.name),
            scopes: parse_scopes(row.scopes)?,
            origin: parse_origin(row.origin, row.oauth_client_id)?,
            expires: row.expires_at,
        })
    }
}

impl TryFrom<AccessTokenAuthenticationRow> for AccessTokenAuthentication {
    type Error = AccessTokenRowMappingError;

    fn try_from(row: AccessTokenAuthenticationRow) -> Result<Self, Self::Error> {
        Ok(Self {
            access_token_id: AccessTokenId::from(row.access_token_id),
            user_id: UserId::from(row.user_id),
            scopes: parse_scopes(row.scopes)?,
            origin: parse_origin(row.origin, row.oauth_client_id)?,
            expires: row.expires_at,
        })
    }
}

pub(crate) fn access_token_id_uuid(
    access_token_id: AccessTokenId,
) -> Result<uuid::Uuid, AccessTokenRowMappingError> {
    uuid::Uuid::parse_str(&access_token_id.to_string())
        .map_err(AccessTokenRowMappingError::InvalidIdentifier)
}

pub(crate) fn scope_values(scopes: &HashSet<Scope>) -> Vec<&'static str> {
    scopes.iter().copied().map(Scope::as_str).collect()
}

pub(crate) fn access_token_origin_values(
    access_token: &AccessToken,
) -> Result<(&'static str, Option<uuid::Uuid>), AccessTokenRowMappingError> {
    match access_token.origin() {
        AccessTokenOrigin::User => Ok(("USER", None)),
        AccessTokenOrigin::OAuth { client_id } => uuid::Uuid::parse_str(&client_id.to_string())
            .map(|client_id| ("OAUTH", Some(client_id)))
            .map_err(|_| AccessTokenRowMappingError::MissingOAuthClientId),
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
    expires_at: Option<time::OffsetDateTime>,
}

fn access_token_from_parts(
    state: AccessTokenPersistedState,
) -> Result<AccessToken, AccessTokenRowMappingError> {
    Ok(AccessToken::rehydrate(RehydratedAccessTokenState {
        id: AccessTokenId::from(state.access_token_id),
        hashed_token: HashedRawAccessToken::new(state.token_short, state.token_hash),
        user_id: UserId::from(state.user_id),
        name: AccessTokenName::from(state.name),
        scopes: parse_scopes(state.scopes)?,
        origin: parse_origin(state.origin, state.oauth_client_id)?,
        expires: state.expires_at,
    }))
}

fn parse_scopes(values: Vec<String>) -> Result<HashSet<Scope>, AccessTokenRowMappingError> {
    values
        .into_iter()
        .map(|value| match value.as_str() {
            "product-listings:write" => Ok(Scope::ProductListingsWrite),
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

fn parse_origin(
    origin: String,
    oauth_client_id: Option<uuid::Uuid>,
) -> Result<AccessTokenOrigin, AccessTokenRowMappingError> {
    match origin.as_str() {
        "USER" if oauth_client_id.is_none() => Ok(AccessTokenOrigin::User),
        "USER" => Err(AccessTokenRowMappingError::UnexpectedOAuthClientId),
        "OAUTH" => oauth_client_id
            .map(|client_id| AccessTokenOrigin::OAuth {
                client_id: client_id.into(),
            })
            .ok_or(AccessTokenRowMappingError::MissingOAuthClientId),
        _ => Err(AccessTokenRowMappingError::InvalidOrigin(origin)),
    }
}
