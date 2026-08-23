use application::operation_context::{
    CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
};
use std::collections::BTreeSet;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestMetadata {
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
}

impl RequestMetadata {
    pub fn new(request_id: impl Into<RequestId>, correlation_id: impl Into<CorrelationId>) -> Self {
        Self {
            request_id: request_id.into(),
            correlation_id: correlation_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportPrincipal {
    Anonymous,
    User {
        user_id: UserId,
        auth_method: AuthMethod,
        capabilities: BTreeSet<CredentialCapability>,
    },
}

impl TransportPrincipal {
    pub fn to_service_principal(&self) -> Principal {
        match self {
            TransportPrincipal::Anonymous => Principal::Anonymous,
            TransportPrincipal::User {
                user_id,
                auth_method: AuthMethod::CognitoJwt,
                ..
            } => Principal::User(*user_id),
            TransportPrincipal::User {
                user_id,
                auth_method: AuthMethod::AuraAccessToken,
                capabilities,
            } => Principal::DelegatedUser {
                user_id: *user_id,
                capabilities: capabilities.clone(),
            },
        }
    }

    pub fn operation_context(&self, metadata: RequestMetadata) -> OperationContext {
        OperationContext {
            principal: self.to_service_principal(),
            request_id: metadata.request_id,
            correlation_id: metadata.correlation_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthMethod {
    CognitoJwt,
    AuraAccessToken,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("authorization header is missing")]
    MissingCredentials,
    #[error("authorization header must use Bearer scheme")]
    InvalidAuthorizationHeader,
    #[error("credential is malformed")]
    MalformedCredentials,
    #[error("credential is invalid, expired, or revoked")]
    InvalidCredentials,
    #[error("claim '{0}' is missing")]
    MissingClaim(&'static str),
    #[error("claim '{0}' has invalid type")]
    InvalidClaimType(&'static str),
    #[error("JWKS key id was not found")]
    JwksKeyNotFound,
    #[error("failed to fetch JWKS: {0}")]
    JwksFetch(String),
    #[error("auth service is temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("auth service failed internally: {0}")]
    Internal(String),
}

#[async_trait::async_trait]
pub trait TokenAuthenticator: Send + Sync {
    async fn authenticate(
        &self,
        bearer_token: &str,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError>;
}

pub fn operation_context(
    principal: &TransportPrincipal,
    metadata: RequestMetadata,
) -> OperationContext {
    principal.operation_context(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> RequestMetadata {
        RequestMetadata::new("req-1", "corr-1")
    }

    #[test]
    fn should_hold_request_metadata_when_created() {
        let metadata = RequestMetadata::new("req-1", "corr-1");

        assert_eq!("req-1", metadata.request_id.as_str());
        assert_eq!("corr-1", metadata.correlation_id.as_str());
    }

    #[test]
    fn should_map_anonymous_transport_principal_to_service_principal() {
        assert_eq!(
            Principal::Anonymous,
            TransportPrincipal::Anonymous.to_service_principal()
        );
    }

    #[test]
    fn should_map_user_transport_principal_to_service_principal() {
        let user_id = UserId::new();
        let principal = TransportPrincipal::User {
            user_id,
            auth_method: AuthMethod::CognitoJwt,
            capabilities: BTreeSet::new(),
        };

        assert_eq!(Principal::User(user_id), principal.to_service_principal());
    }

    #[test]
    fn should_build_operation_context_for_user() {
        let user_id = UserId::new();
        let principal = TransportPrincipal::User {
            user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: BTreeSet::from([CredentialCapability::ProductsWrite]),
        };

        let context = principal.operation_context(metadata());

        assert_eq!(
            Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::from([CredentialCapability::ProductsWrite])
            },
            context.principal
        );
        assert_eq!("req-1", context.request_id.as_str());
        assert_eq!("corr-1", context.correlation_id.as_str());
    }

    #[test]
    fn should_build_operation_context_for_anonymous() {
        let context = operation_context(&TransportPrincipal::Anonymous, metadata());

        assert_eq!(Principal::Anonymous, context.principal);
        assert_eq!("req-1", context.request_id.as_str());
        assert_eq!("corr-1", context.correlation_id.as_str());
    }

    #[test]
    fn should_describe_auth_errors() {
        assert_eq!(
            "authorization header is missing",
            AuthError::MissingCredentials.to_string()
        );

        assert_eq!(
            "auth service failed internally: boom",
            AuthError::Internal("boom".to_owned()).to_string()
        );
    }
}
