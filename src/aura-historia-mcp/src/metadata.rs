use application::operation_context::OperationContext;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Passed to family wrappers alongside their injected service handles.
/// Dropping the HTTP request or reaching its deadline cancels this token.
#[derive(Clone)]
pub struct InvocationContext {
    pub operation: OperationContext,
    pub deadline: Instant,
    pub cancellation: CancellationToken,
    pub(crate) max_response_bytes: usize,
}

#[derive(Clone)]
pub(crate) struct InvocationDeadline {
    pub deadline: Instant,
    pub request_id: String,
}

impl InvocationContext {
    /// Bound application work by the request cancellation and absolute transport deadline.
    pub(crate) async fn execute<T>(
        &self,
        work: impl std::future::Future<Output = T>,
    ) -> Result<T, crate::OperationError> {
        tokio::select! {
            biased;
            _=self.cancellation.cancelled()=>Err(crate::OperationError::Unavailable(application::error::static_error("invocation cancelled"))),
            _=tokio::time::sleep_until(self.deadline)=>Err(crate::OperationError::Unavailable(application::error::static_error("invocation deadline elapsed"))),
            result=work=>Ok(result),
        }
    }
}
