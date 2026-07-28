use crate::user_id::UserId;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    Anonymous,
    User(UserId),
    Service(String),
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationContext {
    pub principal: Principal,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
}

impl OperationContext {
    pub fn actor_label(&self) -> Option<String> {
        match &self.principal {
            Principal::Anonymous => None,
            Principal::User(user_id) => Some(user_id.to_string()),
            Principal::Service(service_id) => Some(service_id.clone()),
            Principal::System => Some("SYSTEM".to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("authenticated principal required")]
pub struct AuthenticationRequired;

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
            Principal::Service(_) => "service",
            Principal::System => "system",
        }
    }

    pub fn actor_id(&self) -> Option<String> {
        match self {
            Principal::Anonymous | Principal::System => None,
            Principal::User(user_id) => Some(user_id.to_string()),
            Principal::Service(service_id) => Some(service_id.clone()),
        }
    }

    pub fn require_authenticated(&self) -> Result<&Self, AuthenticationRequired> {
        match self {
            Principal::Anonymous => Err(AuthenticationRequired),
            Principal::User(_) | Principal::Service(_) | Principal::System => Ok(self),
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.require_authenticated().is_ok()
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
    fn should_expose_safe_principal_kind() {
        assert_eq!("anonymous", Principal::Anonymous.kind());
        assert_eq!("system", Principal::System.kind());
    }
}
