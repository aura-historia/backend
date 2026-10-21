//! Opaque Aura credential adapter shared by HTTP transports.
//! Header extraction, challenges, Cognito authentication and runtime composition stay outside.
use application::{
    error::{BoxError, box_error},
    operation_context::{CredentialCapability, OperationContext},
};
use std::collections::{BTreeSet, HashSet};
use user_core::{
    access_token::{InvalidRawTokenError, RawAccessToken, Scope},
    user_id::UserId,
};
use user_service::use_cases::{
    AuthenticateAccessTokenError, AuthenticateAccessTokenRequest, AuthenticateAccessTokenUseCase,
};

pub struct AuthenticatedAccessToken {
    pub user_id: UserId,
    pub capabilities: BTreeSet<CredentialCapability>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenAuthenticationError {
    #[error("credential is malformed")]
    MalformedCredentials {
        #[source]
        source: InvalidRawTokenError,
    },
    #[error("credential is invalid, expired, or revoked")]
    InvalidCredentials,
    #[error("access token store is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("access token authentication failed internally")]
    Internal {
        #[source]
        source: BoxError,
    },
}

pub async fn authenticate_access_token(
    use_case: &dyn AuthenticateAccessTokenUseCase,
    bearer_token: &str,
    context: &OperationContext,
) -> Result<AuthenticatedAccessToken, AccessTokenAuthenticationError> {
    let token = RawAccessToken::try_from(bearer_token.to_owned())
        .map_err(|source| AccessTokenAuthenticationError::MalformedCredentials { source })?;
    let result = use_case
        .execute(
            context,
            AuthenticateAccessTokenRequest {
                hashed_token: token.into(),
            },
        )
        .await
        .map_err(map_error)?;
    Ok(AuthenticatedAccessToken {
        user_id: result.user_id,
        capabilities: capabilities_from_scopes(&result.scopes),
    })
}

fn map_error(error: AuthenticateAccessTokenError) -> AccessTokenAuthenticationError {
    match error {
        AuthenticateAccessTokenError::NotFound | AuthenticateAccessTokenError::Expired => {
            AccessTokenAuthenticationError::InvalidCredentials
        }
        AuthenticateAccessTokenError::TemporarilyUnavailable { .. } => {
            AccessTokenAuthenticationError::TemporarilyUnavailable {
                source: box_error(error),
            }
        }
        AuthenticateAccessTokenError::Conflict { .. }
        | AuthenticateAccessTokenError::InvalidPersistedState { .. }
        | AuthenticateAccessTokenError::Internal { .. } => {
            AccessTokenAuthenticationError::Internal {
                source: box_error(error),
            }
        }
    }
}

fn capabilities_from_scopes(scopes: &HashSet<Scope>) -> BTreeSet<CredentialCapability> {
    scopes.iter().copied().map(credential_capability).collect()
}

fn credential_capability(scope: Scope) -> CredentialCapability {
    match scope {
        Scope::ProductListingsWrite => CredentialCapability::ProductListingsWrite,
        Scope::ListingSourcesWrite => CredentialCapability::ListingSourcesWrite,
        Scope::AuctionsRead => CredentialCapability::AuctionsRead,
        Scope::ProductListingsRead => CredentialCapability::ProductListingsRead,
        Scope::AuctionsWrite => CredentialCapability::AuctionsWrite,
        Scope::ListingSourcesRead => CredentialCapability::ListingSourcesRead,
        Scope::PartiesRead => CredentialCapability::PartiesRead,
        Scope::PartiesWrite => CredentialCapability::PartiesWrite,
        Scope::PartnershipApplicationsRead => CredentialCapability::PartnershipApplicationsRead,
        Scope::PartnershipApplicationsWrite => CredentialCapability::PartnershipApplicationsWrite,
        Scope::PartnershipsRead => CredentialCapability::PartnershipsRead,
        Scope::PartnershipsWrite => CredentialCapability::PartnershipsWrite,
        Scope::AdminOverviewRead => CredentialCapability::AdminOverviewRead,
        Scope::SearchFiltersRead => CredentialCapability::SearchFiltersRead,
        Scope::NotificationsRead => CredentialCapability::NotificationsRead,
        Scope::NotificationsWrite => CredentialCapability::NotificationsWrite,
        Scope::UsersRead => CredentialCapability::UsersRead,
        Scope::UsersWrite => CredentialCapability::UsersWrite,
        Scope::AccessTokensRead => CredentialCapability::AccessTokensRead,
        Scope::AccessTokensWrite => CredentialCapability::AccessTokensWrite,
        Scope::SearchFiltersWrite => CredentialCapability::SearchFiltersWrite,
        Scope::WatchlistRead => CredentialCapability::WatchlistRead,
        Scope::WatchlistWrite => CredentialCapability::WatchlistWrite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_map_all_token_scopes_to_credential_capabilities() {
        let cases = [
            (
                Scope::ProductListingsWrite,
                CredentialCapability::ProductListingsWrite,
            ),
            (
                Scope::ListingSourcesWrite,
                CredentialCapability::ListingSourcesWrite,
            ),
            (Scope::AuctionsRead, CredentialCapability::AuctionsRead),
            (
                Scope::ProductListingsRead,
                CredentialCapability::ProductListingsRead,
            ),
            (Scope::AuctionsWrite, CredentialCapability::AuctionsWrite),
            (
                Scope::ListingSourcesRead,
                CredentialCapability::ListingSourcesRead,
            ),
            (Scope::PartiesRead, CredentialCapability::PartiesRead),
            (Scope::PartiesWrite, CredentialCapability::PartiesWrite),
            (
                Scope::PartnershipApplicationsRead,
                CredentialCapability::PartnershipApplicationsRead,
            ),
            (
                Scope::PartnershipApplicationsWrite,
                CredentialCapability::PartnershipApplicationsWrite,
            ),
            (
                Scope::PartnershipsRead,
                CredentialCapability::PartnershipsRead,
            ),
            (
                Scope::PartnershipsWrite,
                CredentialCapability::PartnershipsWrite,
            ),
            (
                Scope::AdminOverviewRead,
                CredentialCapability::AdminOverviewRead,
            ),
            (
                Scope::SearchFiltersRead,
                CredentialCapability::SearchFiltersRead,
            ),
            (
                Scope::NotificationsRead,
                CredentialCapability::NotificationsRead,
            ),
            (
                Scope::NotificationsWrite,
                CredentialCapability::NotificationsWrite,
            ),
            (Scope::UsersRead, CredentialCapability::UsersRead),
            (Scope::UsersWrite, CredentialCapability::UsersWrite),
            (
                Scope::AccessTokensRead,
                CredentialCapability::AccessTokensRead,
            ),
            (
                Scope::AccessTokensWrite,
                CredentialCapability::AccessTokensWrite,
            ),
            (
                Scope::SearchFiltersWrite,
                CredentialCapability::SearchFiltersWrite,
            ),
            (Scope::WatchlistRead, CredentialCapability::WatchlistRead),
            (Scope::WatchlistWrite, CredentialCapability::WatchlistWrite),
        ];

        for (scope, capability) in cases {
            assert_eq!(capability, credential_capability(scope));
        }
        let scopes = cases.iter().map(|(scope, _)| *scope).collect();
        let capabilities = cases
            .iter()
            .map(|(_, capability)| *capability)
            .collect::<BTreeSet<_>>();
        assert_eq!(capabilities, capabilities_from_scopes(&scopes));
    }
}
