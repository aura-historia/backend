//! Kinesis Lambda transport boundary for the DMS CDC router.
//!
//! This module owns only Lambda envelope validation, invocation budgeting, and Kinesis sequence
//! checkpoints. DMS row validation, compact job construction, and SQS publication remain in
//! `cdc`, keeping database and business workflows outside this function.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lambda_events::{
    event::streams::KinesisEventResponse,
    kinesis::{KinesisEncryptionType, KinesisEvent, KinesisEventRecord},
};
use lambda_runtime::{Context, Error, LambdaEvent};
use serde::Deserialize;
use tracing::{info, warn};

use crate::cdc::{CdcFanout, PUBLICATION_TIMEOUT};

const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(30);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const KINESIS_EVENT_SOURCE: &str = "aws:kinesis";
const KINESIS_EVENT_NAME: &str = "aws:kinesis:record";
const KINESIS_EVENT_VERSION: &str = "1.0";
const MAX_FAILURE_ARCHIVE_BYTES: usize = 7 * 1024 * 1024;

/// Decodes the full Kinesis invocation preserved by the Lambda on-failure S3 destination.
///
/// Operators use this before a controlled replay after retaining the original archive object. It
/// deliberately returns the original native Kinesis envelope, retaining source ARN, sequence
/// numbers, DMS schema metadata, and base64 record bytes instead of manufacturing a new job.
pub fn decode_failure_archive(body: &[u8]) -> Result<KinesisEvent, FailureArchiveDecodeError> {
    if body.len() > MAX_FAILURE_ARCHIVE_BYTES {
        return Err(FailureArchiveDecodeError::LimitExceeded);
    }
    let archive: FailureArchive =
        serde_json::from_slice(body).map_err(|_| FailureArchiveDecodeError::Invalid)?;
    // Lambda's S3 on-failure destination stores the original invocation in `payload` as an
    // escaped JSON string, not as a nested `requestPayload` object.
    let event: KinesisEvent =
        serde_json::from_str(&archive.payload).map_err(|_| FailureArchiveDecodeError::Invalid)?;
    if event.records.is_empty()
        || event
            .records
            .iter()
            .any(|record| !valid_sequence_number(&record.kinesis.sequence_number))
    {
        return Err(FailureArchiveDecodeError::Invalid);
    }
    Ok(event)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FailureArchiveDecodeError {
    #[error("CDC router failure archive exceeds its bounded replay size")]
    LimitExceeded,
    #[error("CDC router failure archive is not a valid Kinesis invocation")]
    Invalid,
}

#[derive(Deserialize)]
struct FailureArchive {
    payload: String,
}

/// Derives a bounded router budget from Lambda's actual deadline and reserves time to serialize
/// its partial-batch response.
pub fn invocation_budget(context: &Context) -> Duration {
    Duration::from_millis(context.deadline.saturating_sub(epoch_millis()))
        .min(LAMBDA_INVOCATION_CAP)
        .saturating_sub(RESPONSE_HEADROOM)
}

/// Processes records sequentially and reports only the earliest unconfirmed Kinesis sequence.
///
/// Lambda checkpoints records before that sequence and retries from that sequence onward. The
/// router deliberately stops there: later records were not processed and must never be reported
/// as success. A send failure may be ambiguous, so replay can duplicate an earlier SQS job; the
/// existing compact domain keys and downstream guards own that at-least-once behavior.
pub async fn handler(
    event: LambdaEvent<KinesisEvent>,
    fanout: &CdcFanout,
) -> Result<KinesisEventResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_remaining_budget(event, fanout, budget).await
}

async fn handler_with_remaining_budget(
    event: LambdaEvent<KinesisEvent>,
    fanout: &CdcFanout,
    mut remaining_budget: Duration,
) -> Result<KinesisEventResponse, Error> {
    // Lambda cannot honor a partial failure response without a usable Kinesis sequence number.
    // Validate every sequence before any publication, so a malformed envelope never causes an
    // untruthful checkpoint or an avoidable duplicate of a preceding record.
    for record in &event.payload.records {
        if !valid_sequence_number(&record.kinesis.sequence_number) {
            return Err(Error::from(
                "Kinesis event record has no usable sequence number; fail whole invocation",
            ));
        }
    }

    let record_count = event.payload.records.len();
    for record in event.payload.records {
        let sequence_number = record.kinesis.sequence_number.as_str();
        if remaining_budget.is_zero() {
            return Ok(failed_from(
                sequence_number,
                "insufficient_invocation_budget",
            ));
        }
        let publication_budget = remaining_budget.min(PUBLICATION_TIMEOUT);
        let started_at = std::time::Instant::now();

        let prepared_result = match validate_kinesis_record(&record) {
            Ok(()) => fanout
                .prepare_dms_kinesis_record(&record.kinesis.data)
                .map_err(KinesisRecordError::from),
            Err(error) => Err(error),
        };
        let prepared = match prepared_result {
            Ok(prepared) => prepared,
            Err(error) => {
                warn!(
                    kinesis_sequence_number = sequence_number,
                    outcome = error.category(),
                    "DMS Kinesis record retained for retry or failure archive"
                );
                return Ok(failed_from(sequence_number, error.category()));
            }
        };

        if let Err(error) = fanout.publish_prepared(prepared, publication_budget).await {
            let error = KinesisRecordError::from(error);
            warn!(
                kinesis_sequence_number = sequence_number,
                outcome = error.category(),
                "DMS Kinesis publication is unconfirmed; record retained for retry or failure archive"
            );
            return Ok(failed_from(sequence_number, error.category()));
        }

        remaining_budget = remaining_budget.saturating_sub(started_at.elapsed());
    }

    info!(
        kinesis_record_count = record_count,
        failed_kinesis_record_count = 0,
        "Finished DMS Kinesis CDC router batch"
    );
    Ok(KinesisEventResponse::default())
}

fn failed_from(sequence_number: &str, outcome: &'static str) -> KinesisEventResponse {
    info!(
        kinesis_sequence_number = sequence_number,
        outcome, "DMS Kinesis router returned earliest failed checkpoint"
    );
    let mut response = KinesisEventResponse::default();
    response.add_failure(sequence_number);
    response
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn valid_sequence_number(sequence_number: &str) -> bool {
    !sequence_number.is_empty() && sequence_number.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_kinesis_record(record: &KinesisEventRecord) -> Result<(), KinesisRecordError> {
    if record.event_source.as_deref() != Some(KINESIS_EVENT_SOURCE)
        || record.event_name.as_deref() != Some(KINESIS_EVENT_NAME)
        || record.event_version.as_deref() != Some(KINESIS_EVENT_VERSION)
        || !record
            .event_source_arn
            .as_deref()
            .is_some_and(|arn| arn.starts_with("arn:aws:kinesis:"))
        || !record
            .aws_region
            .as_deref()
            .is_some_and(|region| !region.is_empty())
    {
        return Err(KinesisRecordError::InvalidEnvelope);
    }
    if !matches!(
        record.kinesis.encryption_type,
        KinesisEncryptionType::None | KinesisEncryptionType::Kms
    ) {
        return Err(KinesisRecordError::UnsupportedEncryption);
    }
    Ok(())
}

enum KinesisRecordError {
    InvalidEnvelope,
    UnsupportedEncryption,
    Cdc(crate::cdc::CdcIngestError),
}

impl KinesisRecordError {
    fn category(&self) -> &'static str {
        match self {
            Self::InvalidEnvelope => "invalid_kinesis_envelope",
            Self::UnsupportedEncryption => "unsupported_kinesis_record_encryption",
            Self::Cdc(crate::cdc::CdcIngestError::LimitExceeded) => "cdc_record_limit_exceeded",
            Self::Cdc(crate::cdc::CdcIngestError::InvalidJob) => "invalid_cdc_job",
            Self::Cdc(crate::cdc::CdcIngestError::InvalidJson(_)) => "invalid_dms_json",
            Self::Cdc(crate::cdc::CdcIngestError::Route(_)) => "invalid_dms_contract",
            Self::Cdc(crate::cdc::CdcIngestError::Fanout(_)) => "unconfirmed_sqs_publication",
        }
    }
}

impl From<crate::cdc::CdcIngestError> for KinesisRecordError {
    fn from(value: crate::cdc::CdcIngestError) -> Self {
        Self::Cdc(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{Publisher, WorkerQueueRegistry};
    use aura_historia_jobs::jobs::WorkerQueue;
    use aws_lambda_events::kinesis::KinesisRecord;
    use lambda_runtime::Context;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const DATA: &str =
        include_str!("../tests/fixtures/dms-kinesis/product-listing-event-insert.json");
    const CONTROL: &str =
        include_str!("../tests/fixtures/dms-kinesis/incompatible-control-add-column.json");
    const NOOP: &str =
        include_str!("../tests/fixtures/dms-kinesis/synthetic-notification-delivery-delete.json");

    #[derive(Default)]
    struct Recorder(AtomicUsize);
    #[async_trait::async_trait]
    impl Publisher for Recorder {
        async fn publish(&self, _: &str) -> Result<(), ()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn fanout() -> (CdcFanout, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let mut registry = WorkerQueueRegistry::default();
        for target in WorkerQueue::ALL {
            registry = registry.with_publisher(target, recorder.clone());
        }
        (CdcFanout::new(registry), recorder)
    }

    fn record(sequence: &str, body: &str) -> KinesisEventRecord {
        let mut record = KinesisEventRecord::default();
        record.aws_region = Some("eu-central-1".into());
        record.event_name = Some(KINESIS_EVENT_NAME.into());
        record.event_source = Some(KINESIS_EVENT_SOURCE.into());
        record.event_source_arn =
            Some("arn:aws:kinesis:eu-central-1:123456789012:stream/aura-historia-cdc-test".into());
        record.event_version = Some(KINESIS_EVENT_VERSION.into());
        record.kinesis = KinesisRecord::default();
        record.kinesis.sequence_number = sequence.into();
        record.kinesis.data.extend_from_slice(body.as_bytes());
        record
    }

    fn event(records: Vec<KinesisEventRecord>) -> LambdaEvent<KinesisEvent> {
        let mut payload = KinesisEvent::default();
        payload.records = records;
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(60_000);
        LambdaEvent::new(payload, context)
    }

    #[tokio::test]
    async fn checkpoints_only_the_earliest_unconfirmed_record_and_stops() {
        let (fanout, recorder) = fanout();
        let result = handler(
            event(vec![
                record("100", NOOP),
                record("101", DATA),
                record("102", CONTROL),
                record("103", DATA),
            ]),
            &fanout,
        )
        .await
        .unwrap();
        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            Some("102"),
            result.batch_item_failures[0].item_identifier.as_deref()
        );
        assert_eq!(5, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn missing_sequence_fails_invocation_before_any_send() {
        let (fanout, recorder) = fanout();
        let result = handler(event(vec![record("100", DATA), record("", DATA)]), &fanout).await;
        assert!(result.is_err());
        assert_eq!(0, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn expired_budget_fails_current_sequence_before_publication() {
        let (fanout, recorder) = fanout();
        let result = handler_with_remaining_budget(
            event(vec![record("100", DATA)]),
            &fanout,
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            Some("100"),
            result.batch_item_failures[0].item_identifier.as_deref()
        );
        assert_eq!(0, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn rejects_unsupported_kinesis_envelopes_without_publishing() {
        let (fanout, recorder) = fanout();
        let mut malformed = record("100", DATA);
        malformed.event_source = Some("sequin".into());
        malformed.kinesis.encryption_type = KinesisEncryptionType::Kms;
        let result = handler(event(vec![malformed, record("101", DATA)]), &fanout)
            .await
            .unwrap();
        assert_eq!(
            Some("100"),
            result.batch_item_failures[0].item_identifier.as_deref()
        );
        assert_eq!(0, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn retains_malformed_native_dms_without_falling_back_to_generic_cdc() {
        let (fanout, recorder) = fanout();
        let result = handler(
            event(vec![
                record("100", r#"{"changes":[]}"#),
                record("101", DATA),
            ]),
            &fanout,
        )
        .await
        .unwrap();
        assert_eq!(
            Some("100"),
            result.batch_item_failures[0].item_identifier.as_deref()
        );
        assert_eq!(0, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn accepts_kms_record_and_metadata_scoped_control_before_data() {
        let (fanout, recorder) = fanout();
        let mut encrypted = record("102", DATA);
        encrypted.kinesis.encryption_type = KinesisEncryptionType::Kms;
        let control =
            include_str!("../tests/fixtures/dms-kinesis/synthetic-control-create-table.json");
        let result = handler(event(vec![record("101", control), encrypted]), &fanout)
            .await
            .unwrap();
        assert!(result.batch_item_failures.is_empty());
        assert_eq!(5, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn contradictory_control_does_not_publish_later_records() {
        let (fanout, recorder) = fanout();
        let control =
            include_str!("../tests/fixtures/dms-kinesis/synthetic-control-create-table.json");
        let mut contradictory: serde_json::Value = serde_json::from_str(control).unwrap();
        contradictory["metadata"]["operation"] = serde_json::json!("drop-table");
        contradictory["control"]["operation"] = serde_json::json!("create-table");
        let result = handler(
            event(vec![
                record("100", &contradictory.to_string()),
                record("101", DATA),
            ]),
            &fanout,
        )
        .await
        .unwrap();
        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            Some("100"),
            result.batch_item_failures[0].item_identifier.as_deref()
        );
        assert_eq!(0, recorder.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn reports_first_of_multiple_failures_only() {
        let (fanout, recorder) = fanout();
        let result = handler(
            event(vec![
                record("100", NOOP),
                record("101", CONTROL),
                record("102", CONTROL),
            ]),
            &fanout,
        )
        .await
        .unwrap();
        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            Some("101"),
            result.batch_item_failures[0].item_identifier.as_deref()
        );
        assert_eq!(0, recorder.0.load(Ordering::SeqCst));
    }

    #[test]
    fn archive_decoder_preserves_original_invocation_and_fails_closed() {
        let invocation = event(vec![record("100", DATA)]);
        let archive = serde_json::json!({
            "version": "1.0",
            "timestamp": "2026-09-22T12:00:00Z",
            "requestContext": {
                "requestId": "safe-correlation-id",
                "functionArn": "arn:aws:lambda:eu-central-1:123456789012:function:cdc-router-lambda-test",
                "condition": "RetryAttemptsExhausted",
                "approximateInvokeCount": 4
            },
            "responseContext": {"statusCode": 200},
            "KinesisBatchInfo": {"shardId": "shardId-000000000000", "startSequenceNumber": "100", "endSequenceNumber": "100"},
            "payload": serde_json::to_string(&invocation.payload).unwrap()
        });
        let recovered = decode_failure_archive(&serde_json::to_vec(&archive).unwrap()).unwrap();
        assert_eq!("100", recovered.records[0].kinesis.sequence_number);
        assert_eq!(
            DATA.as_bytes(),
            recovered.records[0].kinesis.data.as_slice()
        );
        assert_eq!(
            Err(FailureArchiveDecodeError::Invalid),
            decode_failure_archive(br#"{"payload": {}}"#)
        );
        assert_eq!(
            Err(FailureArchiveDecodeError::Invalid),
            decode_failure_archive(br#"{"payload":"not-json"}"#)
        );
        assert_eq!(
            Err(FailureArchiveDecodeError::Invalid),
            decode_failure_archive(br#"{"payload":"{\"Records\":[{\"kinesis\":{\"sequenceNumber\":\"not-a-sequence\"}}]}"}"#)
        );
        assert_eq!(
            Err(FailureArchiveDecodeError::Invalid),
            decode_failure_archive(br#"{"payload": "{\"Records\":[]}"}"#)
        );
        assert_eq!(
            Err(FailureArchiveDecodeError::LimitExceeded),
            decode_failure_archive(&vec![b' '; MAX_FAILURE_ARCHIVE_BYTES + 1])
        );
    }
}
