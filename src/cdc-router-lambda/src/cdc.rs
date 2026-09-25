//! Strict DMS record decoding and scoped job fanout. Source data never enters SQS jobs.
use crate::queue::SqsQueue;
pub use aura_historia_cdc_routing::{CdcRouteError, route_change};
use aura_historia_cdc_routing::{CdcTable, canonical_decimal_i64, row_for_operation};
use aura_historia_jobs::{
    jobs::{SearchFilterOperation as CdcOperation, WorkerQueue},
    wire,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub type CdcChange = aura_historia_cdc_routing::CdcChange<CdcOperation>;

pub const MAX_CDC_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_CDC_JOBS: usize = 500;
pub const PUBLICATION_TIMEOUT: Duration = Duration::from_secs(8);
const DMS_KINESIS_SOURCE: &str = "aws-dms-kinesis";

#[async_trait::async_trait]
pub trait Publisher: Send + Sync {
    async fn publish(&self, body: &str) -> Result<(), ()>;
}

#[async_trait::async_trait]
impl Publisher for SqsQueue {
    async fn publish(&self, body: &str) -> Result<(), ()> {
        SqsQueue::publish(self, body).await.map_err(|_| ())
    }
}

#[derive(Default)]
pub struct WorkerQueueRegistry {
    queues: HashMap<WorkerQueue, Arc<dyn Publisher>>,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum WorkerQueueRegistryError {
    #[error("router queue registry has duplicate worker queue {0:?}")]
    Duplicate(WorkerQueue),
    #[error("router queue registry is missing worker queue {0:?}")]
    Missing(WorkerQueue),
}

impl WorkerQueueRegistry {
    pub fn with_all_sqs_queues(
        queues: impl IntoIterator<Item = SqsQueue>,
    ) -> Result<Self, WorkerQueueRegistryError> {
        let mut registry = Self::default();
        for queue in queues {
            let target = queue.config().scope().consumer_queue();
            if registry.queues.insert(target, Arc::new(queue)).is_some() {
                return Err(WorkerQueueRegistryError::Duplicate(target));
            }
        }
        for target in WorkerQueue::ALL {
            if !registry.queues.contains_key(&target) {
                return Err(WorkerQueueRegistryError::Missing(target));
            }
        }
        Ok(registry)
    }

    #[cfg(test)]
    pub(crate) fn with_publisher(
        mut self,
        target: WorkerQueue,
        publisher: Arc<dyn Publisher>,
    ) -> Self {
        self.queues.insert(target, publisher);
        self
    }
}

pub struct CdcFanout {
    registry: WorkerQueueRegistry,
}

pub struct PreparedCdcBatch<'a> {
    publications: Vec<(&'a dyn Publisher, String)>,
}

impl CdcFanout {
    pub fn new(registry: WorkerQueueRegistry) -> Self {
        Self { registry }
    }

    /// Validate and encode the whole record's fanout before the first send.
    pub fn prepare_dms_kinesis_record(
        &self,
        body: &[u8],
    ) -> Result<PreparedCdcBatch<'_>, CdcIngestError> {
        if body.len() > MAX_CDC_BODY_BYTES {
            return Err(CdcIngestError::LimitExceeded);
        }
        let batch = parse_dms_kinesis_record(body).map_err(CdcIngestError::InvalidJson)?;
        if batch.source.as_deref() != Some(DMS_KINESIS_SOURCE) {
            return Err(CdcIngestError::InvalidJob);
        }
        validate_dms_contract(&batch)?;
        let mut publications = Vec::new();
        for change in &batch.changes {
            for job in route_change(change)? {
                if publications.len() == MAX_CDC_JOBS {
                    return Err(CdcIngestError::LimitExceeded);
                }
                let publisher = self
                    .registry
                    .queues
                    .get(&job.target_queue)
                    .ok_or(CdcFanoutError::MissingQueue(job.target_queue))?;
                let body = wire::encode(&job).map_err(|_| CdcIngestError::InvalidJob)?;
                publications.push((publisher.as_ref(), body));
            }
        }
        Ok(PreparedCdcBatch { publications })
    }

    pub async fn publish_prepared(
        &self,
        prepared: PreparedCdcBatch<'_>,
        publication_timeout: Duration,
    ) -> Result<usize, CdcIngestError> {
        let count = prepared.publications.len();
        tokio::time::timeout(publication_timeout, async {
            for (publisher, body) in prepared.publications {
                publisher
                    .publish(&body)
                    .await
                    .map_err(|_| CdcFanoutError::PublicationFailed)?;
            }
            Ok::<_, CdcFanoutError>(())
        })
        .await
        .map_err(|_| CdcFanoutError::PublicationDeadline)??;
        info!(
            changes = 1,
            enqueued = count,
            "CDC source record fanout publication confirmed"
        );
        Ok(count)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct CdcBatch {
    #[serde(default, alias = "id", alias = "webhook_id")]
    pub delivery_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(alias = "events", alias = "records")]
    pub changes: Vec<CdcChange>,
}

fn parse_dms_kinesis_record(body: &[u8]) -> Result<CdcBatch, serde_json::Error> {
    serde_json::from_slice::<DmsKinesisRecord>(body)?.try_into()
}

#[derive(Debug, Deserialize)]
struct DmsKinesisRecord {
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    control: Option<Value>,
    metadata: DmsKinesisMetadata,
}

#[derive(Debug, Deserialize)]
struct DmsKinesisMetadata {
    #[serde(rename = "record-type")]
    record_type: String,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default, rename = "schema-name")]
    schema_name: Option<String>,
    #[serde(default, rename = "table-name")]
    table_name: Option<String>,
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default, rename = "transaction-id")]
    transaction_id: Option<Value>,
    #[serde(default, rename = "transaction-record-id")]
    transaction_record_id: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmsKinesisRecordClassification {
    Trigger,
    Noop,
    InformationalControl,
    IncompatibleSchemaControl,
    Invalid(&'static str),
}

impl TryFrom<DmsKinesisRecord> for CdcBatch {
    type Error = serde_json::Error;

    fn try_from(record: DmsKinesisRecord) -> Result<Self, Self::Error> {
        let DmsKinesisRecord {
            data,
            control,
            metadata,
        } = record;
        let delivery_id = dms_delivery_id(&metadata);
        let commit_timestamp = dms_metadata_value(metadata.timestamp.clone());

        match metadata.record_type.as_str() {
            "data" => {
                if control.is_some() {
                    return Err(dms_record_error("data/control payload"));
                }
                let data = data.ok_or_else(|| dms_record_error("data object"))?;
                let classification = classify_dms_data(&data, &metadata);
                match classification {
                    DmsKinesisRecordClassification::IncompatibleSchemaControl => {
                        return Err(dms_record_error("schema control"));
                    }
                    DmsKinesisRecordClassification::Invalid(reason) => {
                        return Err(dms_record_error(reason));
                    }
                    DmsKinesisRecordClassification::InformationalControl => {
                        return Err(dms_record_error("data classification"));
                    }
                    DmsKinesisRecordClassification::Noop => {
                        return Ok(Self {
                            delivery_id,
                            source: Some(DMS_KINESIS_SOURCE.to_owned()),
                            changes: Vec::new(),
                        });
                    }
                    DmsKinesisRecordClassification::Trigger => {}
                }
                let operation = metadata
                    .operation
                    .as_deref()
                    .and_then(dms_operation)
                    .ok_or_else(|| dms_record_error("operation"))?;
                let (record, old_record) = match operation {
                    CdcOperation::Insert | CdcOperation::Update => (Some(data), None),
                    CdcOperation::Delete => (None, Some(data)),
                };
                Ok(Self {
                    delivery_id,
                    source: Some(DMS_KINESIS_SOURCE.to_owned()),
                    changes: vec![CdcChange {
                        schema: Some("public".to_owned()),
                        table: metadata
                            .table_name
                            .ok_or_else(|| dms_record_error("table"))?,
                        operation,
                        primary_key: BTreeMap::new(),
                        record,
                        old_record,
                        changed_columns: Vec::new(),
                        commit_lsn: None,
                        commit_timestamp,
                    }],
                })
            }
            "control" => {
                if data.is_some() {
                    return Err(dms_record_error("data/control payload"));
                }
                let control = control
                    .as_ref()
                    .and_then(Value::as_object)
                    .ok_or_else(|| dms_record_error("control object"))?;
                // The control object holds table details, never the routing metadata. Refuse a
                // duplicate metadata envelope rather than silently accepting a contradiction.
                if ["operation", "schema-name", "table-name", "record-type"]
                    .iter()
                    .any(|field| control.contains_key(*field))
                {
                    return Err(dms_record_error("control metadata in payload"));
                }
                match classify_dms_control(&metadata, control) {
                    DmsKinesisRecordClassification::InformationalControl => Ok(Self {
                        delivery_id,
                        source: Some(DMS_KINESIS_SOURCE.to_owned()),
                        changes: Vec::new(),
                    }),
                    DmsKinesisRecordClassification::IncompatibleSchemaControl => {
                        Err(dms_record_error("schema control"))
                    }
                    DmsKinesisRecordClassification::Invalid(reason) => {
                        Err(dms_record_error(reason))
                    }
                    DmsKinesisRecordClassification::Trigger
                    | DmsKinesisRecordClassification::Noop => {
                        Err(dms_record_error("control classification"))
                    }
                }
            }
            _ => Err(dms_record_error("record type")),
        }
    }
}

fn dms_delivery_id(metadata: &DmsKinesisMetadata) -> Option<String> {
    let transaction_id = dms_metadata_value(metadata.transaction_id.clone());
    let transaction_record_id = dms_metadata_value(metadata.transaction_record_id.clone());
    match (transaction_id, transaction_record_id) {
        (Some(transaction_id), Some(transaction_record_id)) => {
            Some(format!("dms:{transaction_id}:{transaction_record_id}"))
        }
        (Some(transaction_id), None) => Some(format!("dms:{transaction_id}")),
        (None, Some(transaction_record_id)) => Some(format!("dms:record:{transaction_record_id}")),
        (None, None) => None,
    }
}

fn classify_dms_data(
    data: &Value,
    metadata: &DmsKinesisMetadata,
) -> DmsKinesisRecordClassification {
    if !data.is_object() {
        return DmsKinesisRecordClassification::Invalid("data object");
    }
    if metadata.schema_name.as_deref() != Some("public") {
        return DmsKinesisRecordClassification::Invalid("schema");
    }
    let Some(operation) = metadata.operation.as_deref().and_then(dms_operation) else {
        return DmsKinesisRecordClassification::Invalid("operation");
    };
    let Some(table_name) = metadata.table_name.as_deref() else {
        return DmsKinesisRecordClassification::Invalid("table");
    };
    classify_dms_table_operation(table_name, operation)
}

fn classify_dms_control(
    metadata: &DmsKinesisMetadata,
    control: &Map<String, Value>,
) -> DmsKinesisRecordClassification {
    let operation = match metadata.operation.as_deref() {
        None => return DmsKinesisRecordClassification::Invalid("missing control operation"),
        Some("") => return DmsKinesisRecordClassification::Invalid("empty control operation"),
        Some(operation) => operation,
    };
    let schema = match metadata.schema_name.as_deref() {
        None => return DmsKinesisRecordClassification::Invalid("missing control schema"),
        Some("") => return DmsKinesisRecordClassification::Invalid("empty control schema"),
        Some(schema) => schema,
    };
    let table = match metadata.table_name.as_deref() {
        None => return DmsKinesisRecordClassification::Invalid("missing control table"),
        Some("") => return DmsKinesisRecordClassification::Invalid("empty control table"),
        Some(table) => table,
    };
    if schema != "public" || !dms_table_is_selected(table) {
        return DmsKinesisRecordClassification::IncompatibleSchemaControl;
    }
    match operation {
        "create-table" if control.get("table-def").is_some_and(Value::is_object) => {
            DmsKinesisRecordClassification::InformationalControl
        }
        "create-table" => DmsKinesisRecordClassification::Invalid("control table definition"),
        "rename-table" | "drop-table" | "change-columns" | "add-column" | "drop-column"
        | "rename-column" | "column-type-change" => {
            DmsKinesisRecordClassification::IncompatibleSchemaControl
        }
        _ => DmsKinesisRecordClassification::Invalid("unsupported control operation"),
    }
}

fn classify_dms_change(change: &CdcChange) -> DmsKinesisRecordClassification {
    if change.schema.as_deref() != Some("public") {
        return DmsKinesisRecordClassification::Invalid("schema");
    }
    classify_dms_table_operation(&change.table, change.operation)
}

fn classify_dms_table_operation(
    table: &str,
    operation: CdcOperation,
) -> DmsKinesisRecordClassification {
    match (CdcTable::from(table), operation) {
        (CdcTable::ProductListingEvents, CdcOperation::Insert)
        | (CdcTable::ProductListingRawRevisions, CdcOperation::Insert)
        | (CdcTable::SearchFilters, _)
        | (CdcTable::SearchFilterMatches, CdcOperation::Insert)
        | (CdcTable::NotificationDeliveries, CdcOperation::Insert) => {
            DmsKinesisRecordClassification::Trigger
        }
        (CdcTable::ProductListingEvents, _)
        | (CdcTable::ProductListingRawRevisions, _)
        | (CdcTable::SearchFilterMatches, _)
        | (CdcTable::NotificationDeliveries, _) => DmsKinesisRecordClassification::Noop,
        _ => DmsKinesisRecordClassification::Invalid("table or operation"),
    }
}

fn dms_table_is_selected(table: &str) -> bool {
    matches!(
        CdcTable::from(table),
        CdcTable::ProductListingEvents
            | CdcTable::ProductListingRawRevisions
            | CdcTable::SearchFilters
            | CdcTable::SearchFilterMatches
            | CdcTable::NotificationDeliveries
    )
}

fn dms_operation(value: &str) -> Option<CdcOperation> {
    match value {
        "insert" => Some(CdcOperation::Insert),
        "update" => Some(CdcOperation::Update),
        "delete" => Some(CdcOperation::Delete),
        _ => None,
    }
}

fn dms_metadata_value(value: Option<Value>) -> Option<String> {
    value.map(|value| match value {
        Value::String(value) => value,
        other => other.to_string(),
    })
}

fn dms_record_error(message: &'static str) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}
fn validate_dms_contract(batch: &CdcBatch) -> Result<(), CdcRouteError> {
    for change in &batch.changes {
        match classify_dms_change(change) {
            DmsKinesisRecordClassification::Trigger => {}
            DmsKinesisRecordClassification::Noop
            | DmsKinesisRecordClassification::InformationalControl => continue,
            DmsKinesisRecordClassification::IncompatibleSchemaControl => {
                return Err(CdcRouteError::InvalidSourceContract("schema control"));
            }
            DmsKinesisRecordClassification::Invalid(reason) => {
                return Err(CdcRouteError::InvalidSourceContract(reason));
            }
        }

        let required_columns = match (change.table.as_str(), change.operation) {
            ("product_listing_events", CdcOperation::Insert) => [
                "event_id",
                "product_listing_id",
                "event_type",
                "event_group",
                "event_type_schema_version",
                "payload",
            ]
            .as_slice(),
            ("product_listing_raw_revisions", CdcOperation::Insert) => [
                "product_listing_raw_stream_id",
                "product_listing_raw_revision_id",
                "revision",
            ]
            .as_slice(),
            (
                "search_filters",
                CdcOperation::Insert | CdcOperation::Update | CdcOperation::Delete,
            ) => ["user_search_filter_id", "user_id", "version"].as_slice(),
            ("search_filter_matches", CdcOperation::Insert) => [
                "user_id",
                "user_search_filter_id",
                "product_listing_id",
                "origin_event_id",
            ]
            .as_slice(),
            ("notification_deliveries", CdcOperation::Insert) => {
                ["notification_delivery_id"].as_slice()
            }
            _ => return Err(CdcRouteError::InvalidSourceContract("table or operation")),
        };

        let row = row_for_operation(change)?;
        require_dms_columns(row, required_columns)?;
        match change.table.as_str() {
            "product_listing_raw_revisions" => validate_dms_positive_decimal(row, "revision")?,
            "search_filters" => validate_dms_positive_decimal(row, "version")?,
            _ => {}
        }
    }
    Ok(())
}

fn require_dms_columns(row: &Value, fields: &[&'static str]) -> Result<(), CdcRouteError> {
    let row = row
        .as_object()
        .ok_or(CdcRouteError::InvalidSourceContract("data object"))?;
    for &field in fields {
        if !row.contains_key(field) {
            return Err(CdcRouteError::MissingColumn(field));
        }
    }
    Ok(())
}

fn validate_dms_positive_decimal(row: &Value, field: &'static str) -> Result<(), CdcRouteError> {
    let value = row
        .get(field)
        .and_then(Value::as_str)
        .ok_or(CdcRouteError::InvalidSourceContract("decimal string"))?;
    let value = canonical_decimal_i64(value)
        .ok_or(CdcRouteError::InvalidSourceContract("decimal value"))?;
    if value <= 0 {
        return Err(CdcRouteError::InvalidSourceContract(
            "positive decimal value",
        ));
    }
    Ok(())
}
#[derive(thiserror::Error, Debug)]
pub enum CdcIngestError {
    #[error("CDC batch exceeds bounded ingress limits")]
    LimitExceeded,
    #[error("CDC job metadata is invalid")]
    InvalidJob,
    #[error("invalid CDC JSON")]
    InvalidJson(#[source] serde_json::Error),
    #[error(transparent)]
    Route(#[from] CdcRouteError),
    #[error(transparent)]
    Fanout(#[from] CdcFanoutError),
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CdcFanoutError {
    #[error("SQS publication failed")]
    PublicationFailed,
    #[error("CDC publication deadline exceeded")]
    PublicationDeadline,
    #[error("worker queue is not registered: {0:?}")]
    MissingQueue(WorkerQueue),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct RecordingPublisher {
        bodies: Mutex<Vec<String>>,
        attempts: AtomicUsize,
        fail_at: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Publisher for RecordingPublisher {
        async fn publish(&self, body: &str) -> Result<(), ()> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt == self.fail_at.load(Ordering::SeqCst) {
                return Err(());
            }
            self.bodies.lock().unwrap().push(body.to_owned());
            Ok(())
        }
    }

    fn fanout() -> (CdcFanout, Arc<RecordingPublisher>) {
        let publisher = Arc::new(RecordingPublisher::default());
        let mut registry = WorkerQueueRegistry::default();
        for queue in WorkerQueue::ALL {
            registry = registry.with_publisher(queue, publisher.clone());
        }
        (CdcFanout::new(registry), publisher)
    }

    #[tokio::test]
    async fn publishes_schema_two_compact_jobs_only_after_full_record_validation() {
        let (router, publisher) = fanout();
        let fixtures: &[(&str, usize)] = &[
            (
                include_str!("../tests/fixtures/dms-kinesis/product-listing-event-insert.json"),
                5,
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/raw-revision-insert-max-version.json"),
                1,
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/search-filter-insert-max-version.json"),
                1,
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/search-filter-update.json"),
                1,
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/synthetic-search-filter-delete.json"),
                1,
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/search-filter-match-insert.json"),
                1,
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/notification-delivery-insert.json"),
                1,
            ),
        ];
        for &(fixture, expected) in fixtures {
            let prepared = router
                .prepare_dms_kinesis_record(fixture.as_bytes())
                .unwrap();
            assert_eq!(
                expected,
                router
                    .publish_prepared(prepared, PUBLICATION_TIMEOUT)
                    .await
                    .unwrap()
            );
        }
        let bodies = publisher.bodies.lock().unwrap();
        assert_eq!(11, bodies.len());
        for body in bodies.iter() {
            let job: Value = serde_json::from_str(body).unwrap();
            assert_eq!(2, job["schema_version"]);
            assert!(job["idempotency_key"].is_string());
            assert!(job["ordering_key"].is_string());
            assert!(body.len() <= wire::MAX_JOB_BYTES);
            assert!(!body.contains("fixture-source-id"));
        }
    }

    #[tokio::test]
    async fn acknowledges_informational_controls_and_selected_non_trigger_operations() {
        let (router, publisher) = fanout();
        for fixture in [
            include_str!("../tests/fixtures/dms-kinesis/synthetic-control-create-table.json"),
            include_str!(
                "../tests/fixtures/dms-kinesis/synthetic-notification-delivery-delete.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/synthetic-search-filter-match-feedback-update.json"
            ),
        ] {
            let prepared = router
                .prepare_dms_kinesis_record(fixture.as_bytes())
                .unwrap();
            assert_eq!(
                0,
                router
                    .publish_prepared(prepared, PUBLICATION_TIMEOUT)
                    .await
                    .unwrap()
            );
        }
        assert!(publisher.bodies.lock().unwrap().is_empty());
    }

    #[test]
    fn rejects_incompatible_control_and_invalid_versions_before_publication() {
        let (router, publisher) = fanout();
        for fixture in [
            include_str!("../tests/fixtures/dms-kinesis/incompatible-control-add-column.json"),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-numeric-version.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-delete-missing-user.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-delete-numeric-version.json"
            ),
        ] {
            assert!(
                router
                    .prepare_dms_kinesis_record(fixture.as_bytes())
                    .is_err()
            );
        }
        let valid = include_str!("../tests/fixtures/dms-kinesis/product-listing-event-insert.json");
        let mut registry = WorkerQueueRegistry::default();
        registry =
            registry.with_publisher(WorkerQueue::ProductListingOpenSearch, publisher.clone());
        let missing_destination = CdcFanout::new(registry);
        assert!(
            missing_destination
                .prepare_dms_kinesis_record(valid.as_bytes())
                .is_err()
        );
        assert!(publisher.bodies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unconfirmed_partial_publication_retries_with_identical_domain_keys() {
        let (router, publisher) = fanout();
        publisher.fail_at.store(2, Ordering::SeqCst);
        let record =
            include_str!("../tests/fixtures/dms-kinesis/product-listing-event-insert.json");
        let prepared = router
            .prepare_dms_kinesis_record(record.as_bytes())
            .unwrap();
        assert!(
            router
                .publish_prepared(prepared, PUBLICATION_TIMEOUT)
                .await
                .is_err()
        );
        publisher.fail_at.store(0, Ordering::SeqCst);
        let prepared = router
            .prepare_dms_kinesis_record(record.as_bytes())
            .unwrap();
        assert_eq!(
            5,
            router
                .publish_prepared(prepared, PUBLICATION_TIMEOUT)
                .await
                .unwrap()
        );
        let bodies = publisher.bodies.lock().unwrap();
        assert_eq!(6, bodies.len());
        assert_eq!(bodies[0], bodies[1]);
    }

    fn published_jobs(fixture: &str) -> Vec<Value> {
        let (router, publisher) = fanout();
        let prepared = router
            .prepare_dms_kinesis_record(fixture.as_bytes())
            .unwrap();
        // Use the router's real wire encoding, not a native-worker route result.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(router.publish_prepared(prepared, PUBLICATION_TIMEOUT))
            .unwrap();
        let bodies = publisher.bodies.lock().unwrap();
        bodies
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect()
    }

    #[test]
    fn synthetic_r6_vectors_preserve_exact_job_identity_and_raw_contract() {
        let product = published_jobs(include_str!(
            "../tests/fixtures/dms-kinesis/product-listing-event-insert.json"
        ));
        assert_eq!(5, product.len());
        for job in &product {
            assert_eq!(
                job["idempotency_key"],
                "product-event:evt_01j0000000e008000000000005"
            );
            assert_eq!(job["ordering_key"], "product:pl_01j0000000e008000000000004");
            assert_eq!(job["payload"]["event_id"], "evt_01j0000000e008000000000005");
        }

        let raw_fixture =
            include_str!("../tests/fixtures/dms-kinesis/raw-revision-insert-max-version.json");
        assert!(!raw_fixture.contains("source_payload"));
        let raw = published_jobs(raw_fixture);
        assert_eq!(1, raw.len());
        assert_eq!(raw[0]["job_type"], "PRODUCT_LISTING_RAW_REVISION");
        assert_eq!(raw[0]["payload"]["revision"], i64::MAX);
        assert_eq!(
            raw[0]["payload"]["product_listing_raw_stream_id"],
            "prs_01j0000000e008000000000002"
        );
        assert_eq!(
            raw[0]["payload"]["product_listing_raw_revision_id"],
            "prr_01j0000000e008000000000003"
        );
        assert_eq!(
            raw[0]["idempotency_key"],
            "product-listing-raw-revision:prr_01j0000000e008000000000003"
        );
        assert_eq!(
            raw[0]["ordering_key"],
            "product-listing-raw-stream:prs_01j0000000e008000000000002"
        );
        assert!(!raw[0].to_string().contains("source_payload"));
        assert!(!raw[0].to_string().contains("raw_values"));

        for (file, version, operation) in [
            (
                include_str!("../tests/fixtures/dms-kinesis/search-filter-insert-max-version.json"),
                i64::MAX,
                "INSERT",
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/search-filter-update.json"),
                2,
                "UPDATE",
            ),
            (
                include_str!("../tests/fixtures/dms-kinesis/synthetic-search-filter-delete.json"),
                3,
                "DELETE",
            ),
        ] {
            let jobs = published_jobs(file);
            assert_eq!(1, jobs.len());
            assert_eq!(jobs[0]["payload"]["version"], version);
            assert_eq!(jobs[0]["payload"]["operation"], operation);
            assert_eq!(
                jobs[0]["payload"]["user_id"],
                "usr_01j0000000e008000000000001"
            );
            assert_eq!(
                jobs[0]["payload"]["user_search_filter_id"],
                "sf_01j0000000e008000000000006"
            );
            assert_eq!(
                jobs[0]["ordering_key"],
                "search-filter:sf_01j0000000e008000000000006"
            );
            assert_eq!(
                jobs[0]["idempotency_key"],
                format!(
                    "search-filter:sf_01j0000000e008000000000006:{version}:{}",
                    operation.to_ascii_lowercase()
                )
            );
        }
        let matched = published_jobs(include_str!(
            "../tests/fixtures/dms-kinesis/search-filter-match-insert.json"
        ));
        assert_eq!(matched.len(), 1);
        assert_eq!(
            matched[0]["idempotency_key"],
            "search-filter-match:usr_01j0000000e008000000000001:sf_01j0000000e008000000000006:pl_01j0000000e008000000000004:evt_01j0000000e008000000000005"
        );
        assert_eq!(
            matched[0]["ordering_key"],
            "user:usr_01j0000000e008000000000001"
        );
        let delivery = published_jobs(include_str!(
            "../tests/fixtures/dms-kinesis/notification-delivery-insert.json"
        ));
        assert_eq!(delivery.len(), 1);
        assert_eq!(
            delivery[0]["idempotency_key"],
            "notification-delivery:nd_01j0000000e008000000000007"
        );
        assert_eq!(
            delivery[0]["ordering_key"],
            "notification-delivery:nd_01j0000000e008000000000007"
        );
    }

    #[test]
    fn synthetic_dms_decimal_and_delete_vectors_fail_closed() {
        let (router, publisher) = fanout();
        for fixture in [
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-delete-missing-user.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-delete-invalid-user.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-delete-numeric-version.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-delete-zero-version.json"
            ),
            include_str!(
                "../tests/fixtures/dms-kinesis/invalid-search-filter-numeric-version.json"
            ),
        ] {
            assert!(
                router
                    .prepare_dms_kinesis_record(fixture.as_bytes())
                    .is_err()
            );
        }
        for fixture in [
            include_str!("../tests/fixtures/dms-kinesis/search-filter-insert-max-version.json"),
            include_str!("../tests/fixtures/dms-kinesis/raw-revision-insert-max-version.json"),
        ] {
            for value in ["1e3", "001", "+1", "0", "-1", "-0"] {
                let mut record: Value = serde_json::from_str(fixture).unwrap();
                let field = if record["metadata"]["table-name"] == "search_filters" {
                    "version"
                } else {
                    "revision"
                };
                record["data"][field] = Value::String(value.into());
                assert!(
                    router
                        .prepare_dms_kinesis_record(record.to_string().as_bytes())
                        .is_err(),
                    "accepted {field}={value}"
                );
            }
        }
        assert!(publisher.bodies.lock().unwrap().is_empty());
    }

    #[test]
    fn synthetic_dms_control_metadata_and_table_matrix_fail_closed() {
        let (router, publisher) = fanout();
        let base: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/dms-kinesis/synthetic-control-create-table.json"
        ))
        .unwrap();
        for operation in [
            "rename-table",
            "drop-table",
            "change-columns",
            "add-column",
            "drop-column",
            "rename-column",
            "column-type-change",
            "unknown-control",
        ] {
            let mut record = base.clone();
            record["metadata"]["operation"] = Value::String(operation.into());
            assert!(
                router
                    .prepare_dms_kinesis_record(record.to_string().as_bytes())
                    .is_err()
            );
        }
        for field in ["operation", "schema-name", "table-name"] {
            for replacement in [
                None,
                Some(Value::String(String::new())),
                Some(Value::Number(7.into())),
            ] {
                let mut record = base.clone();
                match replacement {
                    Some(value) => record["metadata"][field] = value,
                    None => {
                        record["metadata"].as_object_mut().unwrap().remove(field);
                    }
                }
                assert!(
                    router
                        .prepare_dms_kinesis_record(record.to_string().as_bytes())
                        .is_err(),
                    "accepted {field}"
                );
            }
        }
        for (field, value) in [("schema-name", "other"), ("table-name", "unknown_table")] {
            let mut record = base.clone();
            record["metadata"][field] = Value::String(value.into());
            assert!(
                router
                    .prepare_dms_kinesis_record(record.to_string().as_bytes())
                    .is_err()
            );
        }
        for control in [
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!({"operation": "create-table", "table-def": {}}),
        ] {
            let mut record = base.clone();
            record["control"] = control;
            assert!(
                router
                    .prepare_dms_kinesis_record(record.to_string().as_bytes())
                    .is_err()
            );
        }
        for (kind, table) in [
            ("unknown", "search_filters"),
            ("data", "unknown_table"),
            ("data", "product_listings"),
        ] {
            let record = serde_json::json!({"data": {}, "metadata": {"record-type": kind, "operation": "insert", "schema-name": "public", "table-name": table}});
            assert!(
                router
                    .prepare_dms_kinesis_record(record.to_string().as_bytes())
                    .is_err()
            );
        }
        for record in [
            serde_json::json!({"data": null, "metadata": {"record-type": "data", "operation": "insert", "schema-name": "public", "table-name": "notification_deliveries"}}),
            serde_json::json!({"data": {}, "control": {"table-def": {}}, "metadata": {"record-type": "control", "operation": "create-table", "schema-name": "public", "table-name": "search_filters"}}),
        ] {
            assert!(
                router
                    .prepare_dms_kinesis_record(record.to_string().as_bytes())
                    .is_err()
            );
        }
        assert!(publisher.bodies.lock().unwrap().is_empty());
    }

    #[test]
    fn distinguishes_missing_empty_and_unsupported_control_metadata() {
        let base: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/dms-kinesis/synthetic-control-create-table.json"
        ))
        .unwrap();
        for (field, replacement, reason) in [
            ("operation", None, "missing control operation"),
            ("operation", Some(""), "empty control operation"),
            (
                "operation",
                Some("unexpected"),
                "unsupported control operation",
            ),
            ("schema-name", None, "missing control schema"),
            ("schema-name", Some(""), "empty control schema"),
            ("table-name", None, "missing control table"),
            ("table-name", Some(""), "empty control table"),
        ] {
            let mut record = base.clone();
            if let Some(value) = replacement {
                record["metadata"][field] = Value::String(value.into());
            } else {
                record["metadata"].as_object_mut().unwrap().remove(field);
            }
            let error = parse_dms_kinesis_record(record.to_string().as_bytes()).unwrap_err();
            assert_eq!(reason, error.to_string(), "{field}");
        }
    }
}
