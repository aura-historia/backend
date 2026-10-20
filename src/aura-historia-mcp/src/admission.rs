use crate::{AccessClass, OperationDefinition};
use application::operation_context::Principal;
use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Method and tool identity come from the bounded body after routing headers agree.
/// Raw credentials stay inside the gate and must never be logged.
pub struct AdmissionRequest<'a> {
    pub headers: &'a HeaderMap,
    pub method: &'a str,
    pub operation: Option<&'a OperationDefinition>,
    pub deadline: Instant,
    pub cancellation: CancellationToken,
}

/// H_GATE implements direct token validation here, before SDK dispatch and wrapping.
#[async_trait]
pub trait AdmissionGate: Send + Sync {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection>;
}

pub struct AdmissionRejection {
    pub status: StatusCode,
    pub headers: HeaderMap,
}

impl AdmissionRejection {
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
        }
    }
}

/// Foundation has no credential verifier. Supplied credentials fail closed.
pub struct AnonymousGate;

#[async_trait]
impl AdmissionGate for AnonymousGate {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        if request
            .headers
            .contains_key(axum::http::header::AUTHORIZATION)
            || request
                .operation
                .is_some_and(|op| op.policy().access == AccessClass::Protected)
        {
            return Err(AdmissionRejection::new(StatusCode::UNAUTHORIZED));
        }
        Ok(Principal::Anonymous)
    }
}
