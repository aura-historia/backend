//! Scheduled AWS Lambda edge handler for immutable canonical FX snapshots.

use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use fxrate_service::{
    CaptureFxRateSnapshotCommand, CaptureFxRateSnapshotOutcome, CaptureFxRateSnapshotUseCase,
};
use lambda_runtime::LambdaEvent;
use serde_json::Value;
use time::OffsetDateTime;
use tracing::{info, warn};

#[tracing::instrument(
    skip(event, snapshots),
    fields(request_id = %event.context.request_id, event_bridge_event_id = tracing::field::Empty)
)]
pub async fn handler(
    event: LambdaEvent<EventBridgeEvent<Value>>,
    snapshots: &(dyn CaptureFxRateSnapshotUseCase + Send + Sync),
) -> Result<(), lambda_runtime::Error> {
    let context = operation_context(&event);
    let source_event_id = event.payload.id.clone().ok_or_else(|| {
        lambda_runtime::Error::from(std::io::Error::other("scheduled event is missing ID"))
    })?;
    tracing::Span::current().record("event_bridge_event_id", &source_event_id);

    let result = snapshots
        .execute(
            &context,
            CaptureFxRateSnapshotCommand {
                source_event_id,
                captured_at: OffsetDateTime::now_utc(),
            },
        )
        .await;
    match result {
        Ok(result) => {
            match result.outcome {
                CaptureFxRateSnapshotOutcome::Captured {
                    fx_rate_id,
                    generation,
                } => {
                    info!(event = "fxrate.snapshot.captured", fx_rate_id = %fx_rate_id, generation = generation.as_i64());
                }
                CaptureFxRateSnapshotOutcome::Duplicate => {
                    info!(event = "fx_rate_snapshot.duplicate");
                }
            }
            Ok(())
        }
        Err(error) => {
            warn!(error = %error, "failed to capture FX rate snapshot");
            Err(error.into())
        }
    }
}

fn operation_context(event: &LambdaEvent<EventBridgeEvent<Value>>) -> OperationContext {
    let request_id = RequestId::new(event.context.request_id.clone());
    let correlation_id = event
        .payload
        .id
        .as_deref()
        .map(CorrelationId::new)
        .unwrap_or_else(|| CorrelationId::new(request_id.as_str()));
    OperationContext {
        principal: Principal::System,
        request_id,
        correlation_id,
    }
}
