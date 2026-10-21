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
        if self.deadline <= Instant::now() {
            return Err(crate::OperationError::Unavailable(
                application::error::static_error("invocation deadline elapsed"),
            ));
        }
        tokio::select! {
            biased;
            _=self.cancellation.cancelled()=>Err(crate::OperationError::Unavailable(application::error::static_error("invocation cancelled"))),
            _=tokio::time::sleep_until(self.deadline)=>Err(crate::OperationError::Unavailable(application::error::static_error("invocation deadline elapsed"))),
            result=work=>Ok(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test(start_paused = true)]
    async fn should_not_poll_ready_work_when_absolute_deadline_has_elapsed() {
        let context = InvocationContext {
            operation: OperationContext {
                principal: Principal::Anonymous,
                request_id: RequestId::new("expired-test"),
                correlation_id: CorrelationId::new("expired-test"),
            },
            deadline: Instant::now(),
            cancellation: CancellationToken::new(),
            max_response_bytes: crate::MAX_RESPONSE_BYTES,
        };
        let polled = AtomicBool::new(false);
        let result = context
            .execute(async { polled.store(true, Ordering::SeqCst) })
            .await;
        assert!(!polled.load(Ordering::SeqCst));
        let error = result.expect_err("elapsed deadline must reject work");
        assert!(matches!(error, crate::OperationError::Unavailable(_)));
        assert_eq!(
            Some("invocation deadline elapsed".to_owned()),
            std::error::Error::source(&error).map(ToString::to_string),
        );
    }
}
