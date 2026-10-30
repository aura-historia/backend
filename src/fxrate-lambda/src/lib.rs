//! Scheduled AWS Lambda edge handler for immutable canonical FX snapshots.

use application::error::find_source;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_service::{
    CaptureFxRateSnapshotCommand, CaptureFxRateSnapshotError, CaptureFxRateSnapshotOutcome,
    CaptureFxRateSnapshotUseCase, ports::FxRateQuoteProviderError,
};
use lambda_runtime::LambdaEvent;
use platform_lambda_bootstrap::{Dependency, FailureDiagnostic, UNCLASSIFIED_FAILURE};
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
            let failure = capture_failure(&error);
            warn!(
                event = "fxrate.snapshot.capture_failed",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                http_status = failure.http_status(),
                sqlstate = failure.sqlstate(),
                "failed to capture FX rate snapshot"
            );
            Err(error.into())
        }
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn capture_failure(error: &CaptureFxRateSnapshotError) -> FailureDiagnostic {
    use CaptureFxRateSnapshotError as E;
    match error {
        E::ServiceOrSystemPrincipalRequired => FailureDiagnostic::rejected("capture", "forbidden"),
        E::SourceEventIdRequired => FailureDiagnostic::rejected("capture", "invalid_input"),
        E::QuoteFetchFailed { .. } => quote_fetch_failure(error),
        E::InvalidQuotes { .. } => FailureDiagnostic::new(
            "quote_validation",
            Dependency::FxRatesApi,
            "response_invalid",
            None,
        ),
        E::BeginTransactionFailed { .. } => FailureDiagnostic::postgres("snapshot_begin", error),
        E::SnapshotInsertFailed { .. } => FailureDiagnostic::postgres("snapshot_insert", error),
        E::CapturedAtNotMonotonic => FailureDiagnostic::new(
            "snapshot_insert",
            Dependency::Postgres,
            "captured_at_not_monotonic",
            None,
        ),
        E::CommitTransactionFailed { .. } => FailureDiagnostic::postgres("snapshot_commit", error),
    }
}

/// Classifies the provider call by the HTTP client error in the chain. Without one, the adapter
/// rejected the provider response itself, which its port variant still names.
fn quote_fetch_failure(error: &CaptureFxRateSnapshotError) -> FailureDiagnostic {
    let http = find_source::<reqwest::Error>(error);
    let category = match http {
        Some(http) if http.is_timeout() => "timeout",
        Some(http) if http.status().is_some() => "http_status",
        Some(http) if http.is_decode() => "response_invalid",
        Some(http) if http.is_connect() => "connect",
        Some(_) => "transport",
        None => match find_source::<FxRateQuoteProviderError>(error) {
            Some(FxRateQuoteProviderError::InvalidResponse { .. }) => "response_invalid",
            Some(FxRateQuoteProviderError::RequestFailed { .. }) | None => UNCLASSIFIED_FAILURE,
        },
    };
    FailureDiagnostic::new(
        "quote_fetch",
        Dependency::FxRatesApi,
        category,
        http.and_then(reqwest::Error::status)
            .map(|status| status.as_u16()),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use fxrate_service::CaptureFxRateSnapshotResult;
    use lambda_runtime::Context;
    use std::sync::Mutex;
    use test_api::CapturedLogs;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    struct Failing(Mutex<Option<CaptureFxRateSnapshotError>>);

    #[async_trait::async_trait]
    impl CaptureFxRateSnapshotUseCase for Failing {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CaptureFxRateSnapshotCommand,
        ) -> Result<CaptureFxRateSnapshotResult, CaptureFxRateSnapshotError> {
            Err(self.0.lock().unwrap().take().expect("one execution"))
        }
    }

    fn event() -> LambdaEvent<EventBridgeEvent<Value>> {
        let mut payload = EventBridgeEvent::default();
        payload.id = Some("event-1".to_owned());
        LambdaEvent::new(payload, Context::default())
    }

    /// The provider client error for one HTTP response with `status` and a sentinel body.
    async fn provider_status_error(status: u16) -> reqwest::Error {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status} failed\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SENTINEL}",
                SENTINEL.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/latest"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err();
        server.await.unwrap();
        error
    }

    #[tokio::test]
    async fn should_record_the_failed_capture_stage_without_its_cause() {
        let cases = [
            (
                CaptureFxRateSnapshotError::QuoteFetchFailed {
                    source: box_error(FxRateQuoteProviderError::RequestFailed {
                        source: box_error(provider_status_error(429).await),
                    }),
                },
                ("quote_fetch", "fxratesapi", "http_status", Some(429)),
            ),
            (
                CaptureFxRateSnapshotError::QuoteFetchFailed {
                    source: box_error(FxRateQuoteProviderError::InvalidResponse {
                        source: application::error::static_error(SENTINEL),
                    }),
                },
                ("quote_fetch", "fxratesapi", "response_invalid", None),
            ),
            (
                CaptureFxRateSnapshotError::SnapshotInsertFailed {
                    source: box_error(sqlx::Error::PoolTimedOut),
                },
                ("snapshot_insert", "postgres", "pool_timed_out", None),
            ),
        ];
        for (error, (stage, dependency, category, http_status)) in cases {
            let snapshots = Failing(Mutex::new(Some(error)));
            let (logs, guard) = CapturedLogs::start();

            let result = handler(event(), &snapshots).await;
            drop(guard);

            assert!(result.is_err());
            let events = logs.events_where("event", "fxrate.snapshot.capture_failed");
            assert_eq!(1, events.len(), "{stage}");
            assert_eq!(Some(stage), events[0]["stage"].as_str());
            assert_eq!(Some(dependency), events[0]["dependency"].as_str());
            assert_eq!(Some(category), events[0]["error_category"].as_str());
            assert_eq!(
                http_status,
                events[0]
                    .get("http_status")
                    .and_then(|status| status.as_u64())
            );
            assert!(!logs.text().contains("SENTINEL"), "{stage}");
        }
    }
}
