use crate::{AccessClass, OperationDefinition};
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
};
use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Method and tool identity come from the bounded body after routing headers agree.
/// Raw credentials stay inside the gate and must never be logged.
pub struct AdmissionRequest<'a> {
    pub headers: &'a HeaderMap,
    pub method: &'a str,
    pub context: &'a OperationContext,
    pub operation: Option<&'a OperationDefinition>,
    pub deadline: Instant,
    pub cancellation: CancellationToken,
}

/// Credentials and operation scope checks run before SDK dispatch and wrapping.
#[async_trait]
pub trait AdmissionGate: Send + Sync {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection>;
}

#[derive(Debug, thiserror::Error)]
#[error("MCP request admission rejected")]
pub struct AdmissionRejection {
    pub status: StatusCode,
    pub headers: HeaderMap,
    #[source]
    source: Option<BoxError>,
}

impl AdmissionRejection {
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            source: None,
        }
    }

    pub(crate) fn with_source(mut self, source: BoxError) -> Self {
        self.source = Some(source);
        self
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
