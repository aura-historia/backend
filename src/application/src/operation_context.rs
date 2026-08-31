pub use credential_core::scope::Scope as CredentialCapability;
use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    Anonymous,
    User(UserId),
    DelegatedUser {
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    },
    Service(String),
    System,
}

impl Principal {
    pub fn label(&self) -> String {
        match self {
            Principal::Anonymous => "ANONYMOUS".to_owned(),
            Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
                user_id.to_string()
            }
            Principal::Service(service_id) => service_id.clone(),
            Principal::System => "SYSTEM".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationContext {
    pub principal: Principal,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("authenticated principal required")]
pub struct AuthenticationRequired;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalAuthorizationError {
    #[error(transparent)]
    AuthenticationRequired(#[from] AuthenticationRequired),
    #[error("principal is not permitted")]
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialAuthorizationError {
    #[error(transparent)]
    AuthenticationRequired(#[from] AuthenticationRequired),
    #[error("credential lacks required capability: {capability}")]
    InsufficientCapability { capability: CredentialCapability },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationAuthorizationError {
    #[error(transparent)]
    AuthenticationRequired(#[from] AuthenticationRequired),
    #[error("operation is not permitted")]
    Forbidden,
    #[error("credential lacks required capability: {capability}")]
    InsufficientCapability { capability: CredentialCapability },
}

impl RequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CorrelationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Principal {
    pub fn kind(&self) -> &'static str {
        match self {
            Principal::Anonymous => "anonymous",
            Principal::User(_) => "user",
            Principal::DelegatedUser { .. } => "delegated_user",
            Principal::Service(_) => "service",
            Principal::System => "system",
        }
    }

    pub fn actor_id(&self) -> Option<String> {
        match self {
            Principal::Anonymous | Principal::System => None,
            Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
                Some(user_id.to_string())
            }
            Principal::Service(service_id) => Some(service_id.clone()),
        }
    }

    pub fn require_authenticated(&self) -> Result<&Self, AuthenticationRequired> {
        match self {
            Principal::Anonymous => Err(AuthenticationRequired),
            Principal::User(_)
            | Principal::DelegatedUser { .. }
            | Principal::Service(_)
            | Principal::System => Ok(self),
        }
    }

    pub fn require(&self) -> PrincipalRequirement<'_> {
        PrincipalRequirement::new(self)
    }

    pub fn require_user(
        &self,
        required_user_id: &UserId,
    ) -> Result<&Self, PrincipalAuthorizationError> {
        self.require().user(required_user_id).check()
    }

    pub fn require_service_or_system(&self) -> Result<&Self, PrincipalAuthorizationError> {
        self.require().service_or_system().check()
    }

    pub fn is_authenticated(&self) -> bool {
        self.require_authenticated().is_ok()
    }

    pub fn require_credential_capability(
        &self,
        capability: CredentialCapability,
    ) -> Result<&Self, CredentialAuthorizationError> {
        self.require_authenticated()?;
        match self {
            Principal::DelegatedUser { capabilities, .. }
                if !capabilities.contains(&capability) =>
            {
                Err(CredentialAuthorizationError::InsufficientCapability { capability })
            }
            Principal::Anonymous => Err(AuthenticationRequired.into()),
            Principal::User(_)
            | Principal::DelegatedUser { .. }
            | Principal::Service(_)
            | Principal::System => Ok(self),
        }
    }

    pub fn is_delegated(&self) -> bool {
        matches!(self, Principal::DelegatedUser { .. })
    }
}

pub struct PrincipalRequirement<'a> {
    principal: &'a Principal,
    allowed: bool,
}

pub struct OperationRequirement<'a> {
    context: &'a OperationContext,
    credential_error: Option<CredentialAuthorizationError>,
    principal_rule_used: bool,
    principal_allowed: bool,
}

impl<'a> PrincipalRequirement<'a> {
    fn new(principal: &'a Principal) -> Self {
        Self {
            principal,
            allowed: false,
        }
    }

    pub fn user(mut self, required_user_id: &UserId) -> Self {
        self.allowed |= matches!(
            self.principal,
            Principal::User(user_id) | Principal::DelegatedUser { user_id, .. }
                if user_id == required_user_id
        );
        self
    }

    pub fn any_user(mut self) -> Self {
        self.allowed |= matches!(
            self.principal,
            Principal::User(_) | Principal::DelegatedUser { .. }
        );
        self
    }

    pub fn service(mut self) -> Self {
        self.allowed |= matches!(self.principal, Principal::Service(_));
        self
    }

    pub fn system(mut self) -> Self {
        self.allowed |= matches!(self.principal, Principal::System);
        self
    }

    pub fn service_or_system(self) -> Self {
        self.service().system()
    }

    pub fn check(self) -> Result<&'a Principal, PrincipalAuthorizationError> {
        if self.allowed {
            Ok(self.principal)
        } else if matches!(self.principal, Principal::Anonymous) {
            Err(AuthenticationRequired.into())
        } else {
            Err(PrincipalAuthorizationError::Forbidden)
        }
    }
}

impl From<CredentialAuthorizationError> for OperationAuthorizationError {
    fn from(error: CredentialAuthorizationError) -> Self {
        match error {
            CredentialAuthorizationError::AuthenticationRequired(error) => {
                OperationAuthorizationError::AuthenticationRequired(error)
            }
            CredentialAuthorizationError::InsufficientCapability { capability } => {
                OperationAuthorizationError::InsufficientCapability { capability }
            }
        }
    }
}

impl From<PrincipalAuthorizationError> for OperationAuthorizationError {
    fn from(error: PrincipalAuthorizationError) -> Self {
        match error {
            PrincipalAuthorizationError::AuthenticationRequired(error) => {
                OperationAuthorizationError::AuthenticationRequired(error)
            }
            PrincipalAuthorizationError::Forbidden => OperationAuthorizationError::Forbidden,
        }
    }
}

impl<'a> OperationRequirement<'a> {
    fn new(context: &'a OperationContext) -> Self {
        Self {
            context,
            credential_error: None,
            principal_rule_used: false,
            principal_allowed: false,
        }
    }

    pub fn credential_capability(mut self, capability: CredentialCapability) -> Self {
        if let Err(error) = self.context.require_credential_capability(capability) {
            self.credential_error.get_or_insert(error);
        }
        self
    }

    pub fn user(mut self, required_user_id: &UserId) -> Self {
        self.principal_rule_used = true;
        self.principal_allowed |= matches!(
            &self.context.principal,
            Principal::User(user_id) | Principal::DelegatedUser { user_id, .. }
                if user_id == required_user_id
        );
        self
    }

    pub fn any_user(mut self) -> Self {
        self.principal_rule_used = true;
        self.principal_allowed |= matches!(
            &self.context.principal,
            Principal::User(_) | Principal::DelegatedUser { .. }
        );
        self
    }

    pub fn service(mut self) -> Self {
        self.principal_rule_used = true;
        self.principal_allowed |= matches!(&self.context.principal, Principal::Service(_));
        self
    }

    pub fn system(mut self) -> Self {
        self.principal_rule_used = true;
        self.principal_allowed |= matches!(&self.context.principal, Principal::System);
        self
    }

    pub fn service_or_system(self) -> Self {
        self.service().system()
    }

    pub fn check(self) -> Result<&'a OperationContext, OperationAuthorizationError> {
        if let Some(error) = self.credential_error {
            return Err(error.into());
        }
        if !self.principal_rule_used || self.principal_allowed {
            Ok(self.context)
        } else if matches!(self.context.principal, Principal::Anonymous) {
            Err(AuthenticationRequired.into())
        } else {
            Err(OperationAuthorizationError::Forbidden)
        }
    }

    pub fn authorize<E>(self) -> Result<(), E>
    where
        E: From<OperationAuthorizationError>,
    {
        self.check().map(drop).map_err(E::from)
    }
}

impl OperationContext {
    pub fn require(&self) -> OperationRequirement<'_> {
        OperationRequirement::new(self)
    }

    pub fn require_user(
        &self,
        required_user_id: &UserId,
    ) -> Result<&Principal, PrincipalAuthorizationError> {
        self.principal.require_user(required_user_id)
    }

    pub fn require_service_or_system(&self) -> Result<&Principal, PrincipalAuthorizationError> {
        self.principal.require_service_or_system()
    }

    pub fn require_credential_capability(
        &self,
        capability: CredentialCapability,
    ) -> Result<&Principal, CredentialAuthorizationError> {
        self.principal.require_credential_capability(capability)
    }
}

impl Display for RequestId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Display for CorrelationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for RequestId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for CorrelationId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for CorrelationId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_anonymous_when_authenticated_principal_required() {
        assert_eq!(
            Err(AuthenticationRequired),
            Principal::Anonymous.require_authenticated()
        );
    }

    #[test]
    fn should_accept_user_when_authenticated_principal_required() {
        let principal = Principal::User(UserId::new());

        assert_eq!(Ok(&principal), principal.require_authenticated());
    }

    #[test]
    fn should_accept_delegated_user_when_authenticated_principal_required() {
        let principal = Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: BTreeSet::new(),
        };

        assert_eq!(Ok(&principal), principal.require_authenticated());
    }

    #[test]
    fn should_render_all_capabilities_as_oauth_scope_strings() {
        for (capability, scope) in [
            (
                CredentialCapability::ProductListingsWrite,
                "product-listings:write",
            ),
            (CredentialCapability::UsersRead, "users:read"),
            (CredentialCapability::UsersWrite, "users:write"),
            (CredentialCapability::AccessTokensRead, "access-tokens:read"),
            (
                CredentialCapability::AccessTokensWrite,
                "access-tokens:write",
            ),
            (
                CredentialCapability::SearchFiltersWrite,
                "search-filters:write",
            ),
            (CredentialCapability::WatchlistRead, "watchlist:read"),
            (CredentialCapability::WatchlistWrite, "watchlist:write"),
        ] {
            assert_eq!(scope, capability.as_scope_str());
            assert_eq!(scope, capability.to_string());
        }
    }

    #[test]
    fn should_use_open_world_capabilities_for_first_party_user() {
        let principal = Principal::User(UserId::new());

        assert_eq!(
            Ok(&principal),
            principal.require_credential_capability(CredentialCapability::ProductListingsWrite)
        );
    }

    #[test]
    fn should_use_closed_world_capabilities_for_delegated_user() {
        let user_id = UserId::new();
        let principal = Principal::DelegatedUser {
            user_id,
            capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
        };

        assert_eq!(
            Ok(&principal),
            principal.require_credential_capability(CredentialCapability::ProductListingsWrite)
        );
        assert_eq!(
            Err(CredentialAuthorizationError::InsufficientCapability {
                capability: CredentialCapability::UsersWrite
            }),
            principal.require_credential_capability(CredentialCapability::UsersWrite)
        );
    }

    #[test]
    fn should_reject_anonymous_when_capability_required() {
        assert_eq!(
            Err(CredentialAuthorizationError::AuthenticationRequired(
                AuthenticationRequired
            )),
            Principal::Anonymous
                .require_credential_capability(CredentialCapability::ProductListingsWrite)
        );
    }

    #[test]
    fn should_require_exact_user_or_delegate() {
        let user_id = UserId::new();
        let delegated = Principal::DelegatedUser {
            user_id,
            capabilities: BTreeSet::new(),
        };

        assert_eq!(Ok(&delegated), delegated.require_user(&user_id));
        assert_eq!(
            Err(PrincipalAuthorizationError::Forbidden),
            delegated.require_user(&UserId::new())
        );
        assert_eq!(
            Err(PrincipalAuthorizationError::AuthenticationRequired(
                AuthenticationRequired
            )),
            Principal::Anonymous.require_user(&user_id)
        );
    }

    #[test]
    fn should_allow_user_or_service_or_system_with_chain() {
        let user_id = UserId::new();
        let user = Principal::User(user_id);
        let service = Principal::Service("svc".to_owned());
        let system = Principal::System;
        let other = Principal::User(UserId::new());

        assert_eq!(
            Ok(&user),
            user.require().user(&user_id).service_or_system().check()
        );
        assert_eq!(
            Ok(&service),
            service.require().user(&user_id).service_or_system().check()
        );
        assert_eq!(
            Ok(&system),
            system.require().user(&user_id).service_or_system().check()
        );
        assert_eq!(
            Err(PrincipalAuthorizationError::Forbidden),
            other.require().user(&user_id).service_or_system().check()
        );
    }

    #[test]
    fn should_allow_operation_when_capability_and_principal_rules_pass() {
        let user_id = UserId::new();
        let context = OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            },
            request_id: RequestId::new("req"),
            correlation_id: CorrelationId::new("corr"),
        };

        assert_eq!(
            Ok(&context),
            context
                .require()
                .credential_capability(CredentialCapability::ProductListingsWrite)
                .user(&user_id)
                .service_or_system()
                .check()
        );
    }

    #[test]
    fn should_reject_operation_when_delegated_user_lacks_capability() {
        let user_id = UserId::new();
        let context = OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::new(),
            },
            request_id: RequestId::new("req"),
            correlation_id: CorrelationId::new("corr"),
        };

        assert_eq!(
            Err(OperationAuthorizationError::InsufficientCapability {
                capability: CredentialCapability::ProductListingsWrite
            }),
            context
                .require()
                .credential_capability(CredentialCapability::ProductListingsWrite)
                .user(&user_id)
                .check()
        );
    }

    #[test]
    fn should_reject_operation_when_user_rule_fails() {
        let context = OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("req"),
            correlation_id: CorrelationId::new("corr"),
        };

        assert_eq!(
            Err(OperationAuthorizationError::Forbidden),
            context.require().user(&UserId::new()).check()
        );
    }

    #[test]
    fn should_reject_operation_when_anonymous() {
        let context = OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("req"),
            correlation_id: CorrelationId::new("corr"),
        };

        assert_eq!(
            Err(OperationAuthorizationError::AuthenticationRequired(
                AuthenticationRequired
            )),
            context.require().any_user().check()
        );
    }

    #[test]
    fn should_authorize_operation_as_unit_result() {
        let context = OperationContext {
            principal: Principal::Service("svc".to_owned()),
            request_id: RequestId::new("req"),
            correlation_id: CorrelationId::new("corr"),
        };

        assert_eq!(
            Ok::<(), OperationAuthorizationError>(()),
            context.require().service_or_system().authorize()
        );
    }

    #[test]
    fn should_expose_safe_principal_kind() {
        assert_eq!("anonymous", Principal::Anonymous.kind());
        assert_eq!("user", Principal::User(UserId::new()).kind());
        assert_eq!(
            "delegated_user",
            Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::new()
            }
            .kind()
        );
        assert_eq!("system", Principal::System.kind());
    }
}
