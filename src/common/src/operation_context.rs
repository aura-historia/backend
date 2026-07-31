use crate::user_id::UserId;
use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialCapability {
    ProductsWrite,
    ShopsManage,
}

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
    #[error("credential lacks required capability: {capability:?}")]
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

impl OperationContext {
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
    fn should_use_open_world_capabilities_for_first_party_user() {
        let principal = Principal::User(UserId::new());

        assert_eq!(
            Ok(&principal),
            principal.require_credential_capability(CredentialCapability::ProductsWrite)
        );
    }

    #[test]
    fn should_use_closed_world_capabilities_for_delegated_user() {
        let user_id = UserId::new();
        let principal = Principal::DelegatedUser {
            user_id,
            capabilities: BTreeSet::from([CredentialCapability::ProductsWrite]),
        };

        assert_eq!(
            Ok(&principal),
            principal.require_credential_capability(CredentialCapability::ProductsWrite)
        );
        assert_eq!(
            Err(CredentialAuthorizationError::InsufficientCapability {
                capability: CredentialCapability::ShopsManage
            }),
            principal.require_credential_capability(CredentialCapability::ShopsManage)
        );
    }

    #[test]
    fn should_reject_anonymous_when_capability_required() {
        assert_eq!(
            Err(CredentialAuthorizationError::AuthenticationRequired(
                AuthenticationRequired
            )),
            Principal::Anonymous.require_credential_capability(CredentialCapability::ProductsWrite)
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
