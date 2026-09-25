use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use aura_historia_cdc_routing as routing;
#[cfg(test)]
use domain_primitives::event_id::EventId;
#[cfg(test)]
use product_listing_core::product_listing_id::ProductListingId;
pub use routing::{CdcRouteError, route_change};
use routing::{
    CdcTable, Operation, RouteOperation, canonical_decimal_i64, notification_delivery_created_job,
    product_event_jobs, product_listing_raw_revision_job, row_for_operation,
    search_filter_changed_job, search_filter_match_created_job,
};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tracing::info;
#[cfg(test)]
use uuid::Uuid;

use crate::{
    InMemoryQueueReceiver, InMemoryQueueSender, QueueConfig, QueueConfigError, in_memory_queue,
};

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct CdcBatch {
    #[serde(default, alias = "id", alias = "webhook_id")]
    pub delivery_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(alias = "events", alias = "records")]
    pub changes: Vec<CdcChange>,
}

pub type CdcChange = routing::CdcChange<CdcOperation>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcOperation {
    Insert,
    Update,
    Delete,
}

pub use crate::jobs::*;

pub const MAX_CDC_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_CDC_CHANGES: usize = 100;
pub const MAX_CDC_JOBS: usize = 500;
pub const PUBLICATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const DMS_KINESIS_SOURCE: &str = "aws-dms-kinesis";

impl Display for CdcOperation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CdcOperation::Insert => write!(formatter, "insert"),
            CdcOperation::Update => write!(formatter, "update"),
            CdcOperation::Delete => write!(formatter, "delete"),
        }
    }
}

impl FromStr for CdcOperation {
    type Err = CdcOperationParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "insert" | "create" => Ok(Self::Insert),
            "update" | "modify" => Ok(Self::Update),
            "delete" | "remove" => Ok(Self::Delete),
            _ => Err(CdcOperationParseError {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("unsupported CDC operation: {value}")]
pub struct CdcOperationParseError {
    value: String,
}

impl RouteOperation for CdcOperation {
    type ParseError = CdcOperationParseError;

    fn parse(value: &str) -> Result<Self, Self::ParseError> {
        Self::from_str(value)
    }

    fn kind(self) -> Operation {
        match self {
            Self::Insert => Operation::Insert,
            Self::Update => Operation::Update,
            Self::Delete => Operation::Delete,
        }
    }
}

fn deserialize_operation<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<CdcOperation, D::Error> {
    let value = String::deserialize(deserializer)?;
    CdcOperation::from_str(&value).map_err(serde::de::Error::custom)
}

#[derive(Debug, Default, Clone)]
pub struct WorkerQueueRegistry {
    queues: HashMap<WorkerQueue, Destination>,
}

#[derive(Debug, Clone)]
enum Destination {
    Memory(InMemoryQueueSender<DomainJob>),
    Sqs(Box<crate::queue::SqsQueue>),
}

enum PreparedPublication<'a> {
    Memory(&'a InMemoryQueueSender<DomainJob>, DomainJob),
    Sqs(&'a crate::queue::SqsQueue, String),
}

/// A fully validated, serialized source-record fanout that has not been published yet.
///
/// This stays inside the worker transport boundary so a Kinesis invocation can retain a
/// source record when its complete fanout is not confirmed.
pub(crate) struct PreparedCdcBatch<'a> {
    changes: usize,
    publications: Vec<PreparedPublication<'a>>,
}

impl PreparedPublication<'_> {
    async fn publish(self) -> Result<(), CdcFanoutError> {
        match self {
            Self::Memory(sender, job) => sender.enqueue(job).await.map_err(Into::into),
            Self::Sqs(queue, body) => queue
                .publish(&body)
                .await
                .map_err(|_| CdcFanoutError::PublicationFailed),
        }
    }
}

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerQueueRegistryError {
    #[error("router queue registry is missing worker queue {0:?}")]
    Missing(WorkerQueue),
    #[error("router queue registry has duplicate worker queue {0:?}")]
    Duplicate(WorkerQueue),
}

impl WorkerQueueRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers exactly one validated Standard SQS queue for every production scope before the
    /// router begins consuming Kinesis. A missing or duplicated destination is a startup error,
    /// not a source record that could be partially fanned out.
    pub fn with_all_sqs_queues(
        queues: impl IntoIterator<Item = crate::queue::SqsQueue>,
    ) -> Result<Self, WorkerQueueRegistryError> {
        let mut registry = Self::new();
        for queue in queues {
            let worker_queue = queue.config().scope().consumer_queue();
            if registry.queues.contains_key(&worker_queue) {
                return Err(WorkerQueueRegistryError::Duplicate(worker_queue));
            }
            registry
                .queues
                .insert(worker_queue, Destination::Sqs(Box::new(queue)));
        }
        for worker_queue in WorkerQueue::ALL {
            if !registry.queues.contains_key(&worker_queue) {
                return Err(WorkerQueueRegistryError::Missing(worker_queue));
            }
        }
        Ok(registry)
    }

    pub fn with_all_queues(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let mut registry = Self::new();
        let mut receivers = WorkerQueueReceivers::new();

        for queue in WorkerQueue::ALL {
            let (sender, receiver) = in_memory_queue::<DomainJob>(config)?;
            registry = registry.with_queue(queue, sender);
            receivers.insert(queue, receiver);
        }

        Ok((registry, receivers))
    }

    pub fn with_queue(
        mut self,
        queue: WorkerQueue,
        sender: InMemoryQueueSender<DomainJob>,
    ) -> Self {
        self.queues.insert(queue, Destination::Memory(sender));
        self
    }

    pub fn with_sqs_queue(mut self, queue: crate::queue::SqsQueue) -> Self {
        self.queues.insert(
            queue.config().scope().consumer_queue(),
            Destination::Sqs(Box::new(queue)),
        );
        self
    }

    fn prepare(&self, job: DomainJob) -> Result<PreparedPublication<'_>, CdcIngestError> {
        job.validate().map_err(|_| CdcIngestError::InvalidJob)?;
        let destination = self
            .queues
            .get(&job.target_queue)
            .ok_or(CdcFanoutError::MissingQueue(job.target_queue))?;
        match destination {
            Destination::Memory(sender) => {
                if sender.sender.is_closed() {
                    return Err(CdcFanoutError::QueueClosed(job.target_queue).into());
                }
                Ok(PreparedPublication::Memory(sender, job))
            }
            Destination::Sqs(queue) => {
                if job.target_queue != queue.config().scope().consumer_queue() {
                    return Err(CdcIngestError::InvalidJob);
                }
                let body = crate::wire::encode(&job).map_err(|_| CdcIngestError::InvalidJob)?;
                Ok(PreparedPublication::Sqs(queue, body))
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct WorkerQueueReceivers {
    receivers: HashMap<WorkerQueue, InMemoryQueueReceiver<DomainJob>>,
}

impl WorkerQueueReceivers {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(
        &mut self,
        queue: WorkerQueue,
        receiver: InMemoryQueueReceiver<DomainJob>,
    ) {
        self.receivers.insert(queue, receiver);
    }

    pub fn take(&mut self, queue: WorkerQueue) -> Option<InMemoryQueueReceiver<DomainJob>> {
        self.receivers.remove(&queue)
    }

    pub async fn recv(&mut self, queue: WorkerQueue) -> Option<DomainJob> {
        self.receivers.get_mut(&queue)?.recv().await
    }

    pub async fn recv_timeout(
        &mut self,
        queue: WorkerQueue,
        duration: std::time::Duration,
    ) -> Result<Option<DomainJob>, tokio::time::error::Elapsed> {
        tokio::time::timeout(duration, self.recv(queue)).await
    }
}

#[derive(Debug, Clone)]
pub struct CdcFanout {
    registry: WorkerQueueRegistry,
    scope: CdcFanoutScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CdcFanoutScope {
    All,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    SearchFilterProjection,
    WatchlistNotification,
    ProductListingContentAssessment,
    ProductListingTranslation,
    ProductListingEmbedding,
    ProductListingOpenSearch,
    ProductListingRawNormalization,
    NotificationDelivery,
}

impl CdcFanout {
    pub fn for_scope(scope: crate::WorkerScope, registry: WorkerQueueRegistry) -> Self {
        use crate::WorkerScope;
        match scope {
            WorkerScope::SearchFilterProjection => Self::search_filter_projection(registry),
            WorkerScope::SearchFilterPercolator => Self::search_filter_percolator(registry),
            WorkerScope::SearchFilterMatchNotification => {
                Self::search_filter_match_notification(registry)
            }
            WorkerScope::WatchlistNotification => Self::watchlist_notification(registry),
            WorkerScope::ProductListingContentAssessment => {
                Self::product_content_assessment(registry)
            }
            WorkerScope::ProductListingTranslation => Self::product_translation(registry),
            WorkerScope::ProductListingEmbedding => Self::product_embedding(registry),
            WorkerScope::ProductListingOpenSearch => Self::product_listing_opensearch(registry),
            WorkerScope::ProductListingRawNormalization => {
                Self::product_listing_raw_normalization(registry)
            }
            WorkerScope::NotificationDelivery => Self::notification_delivery(registry),
        }
    }

    pub fn new(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::All,
        }
    }

    pub fn watchlist_notification(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::WatchlistNotification,
        }
    }

    pub fn search_filter_percolator(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::SearchFilterPercolator,
        }
    }

    pub fn search_filter_match_notification(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::SearchFilterMatchNotification,
        }
    }

    pub fn search_filter_projection(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::SearchFilterProjection,
        }
    }

    pub fn product_content_assessment(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingContentAssessment,
        }
    }

    pub fn product_translation(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingTranslation,
        }
    }

    pub fn product_embedding(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingEmbedding,
        }
    }

    pub fn product_listing_opensearch(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingOpenSearch,
        }
    }

    pub fn product_listing_raw_normalization(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingRawNormalization,
        }
    }

    pub fn notification_delivery(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::NotificationDelivery,
        }
    }

    pub async fn ingest_json(&self, body: &str) -> Result<usize, CdcIngestError> {
        if body.len() > MAX_CDC_BODY_BYTES {
            return Err(CdcIngestError::LimitExceeded);
        }
        let batch = parse_cdc_batch(body).map_err(CdcIngestError::InvalidJson)?;
        self.ingest_batch(&batch).await
    }

    pub async fn ingest_batch(&self, batch: &CdcBatch) -> Result<usize, CdcIngestError> {
        let prepared = self.prepare_batch(batch)?;
        self.publish_prepared(prepared, PUBLICATION_TIMEOUT).await
    }

    fn prepare_batch(&self, batch: &CdcBatch) -> Result<PreparedCdcBatch<'_>, CdcIngestError> {
        if batch.changes.len() > MAX_CDC_CHANGES {
            return Err(CdcIngestError::LimitExceeded);
        }
        let dms_source = batch.source.as_deref() == Some(DMS_KINESIS_SOURCE);
        if dms_source {
            validate_dms_contract(batch)?;
        }
        let mut publications = Vec::new();
        for change in &batch.changes {
            if dms_source {
                match classify_dms_change(change) {
                    DmsKinesisRecordClassification::Trigger => {}
                    DmsKinesisRecordClassification::Noop
                    | DmsKinesisRecordClassification::InformationalControl => continue,
                    DmsKinesisRecordClassification::IncompatibleSchemaControl => {
                        return Err(CdcRouteError::InvalidSourceContract("schema control").into());
                    }
                    DmsKinesisRecordClassification::Invalid(reason) => {
                        return Err(CdcRouteError::InvalidSourceContract(reason).into());
                    }
                }
            }
            if change
                .schema
                .as_deref()
                .is_some_and(|schema| schema != "public")
            {
                return Err(CdcIngestError::InvalidJob);
            }
            for job in self.route_change(change)? {
                if publications.len() == MAX_CDC_JOBS {
                    return Err(CdcIngestError::LimitExceeded);
                }
                publications.push(self.registry.prepare(job)?);
            }
        }
        // Nothing has been published yet. Invalid later changes or missing destinations cannot
        // create a partial fanout. Network failures still can; redelivery is intentionally safe.
        Ok(PreparedCdcBatch {
            changes: batch.changes.len(),
            publications,
        })
    }

    pub(crate) async fn publish_prepared(
        &self,
        prepared: PreparedCdcBatch<'_>,
        publication_timeout: std::time::Duration,
    ) -> Result<usize, CdcIngestError> {
        let PreparedCdcBatch {
            changes,
            publications,
        } = prepared;
        let enqueued = publications.len();
        tokio::time::timeout(publication_timeout, async {
            for publication in publications {
                publication.publish().await?;
            }
            Ok::<_, CdcFanoutError>(())
        })
        .await
        .map_err(|_| CdcFanoutError::PublicationDeadline)??;
        info!(
            changes,
            enqueued, "CDC source record fanout publication confirmed"
        );
        Ok(enqueued)
    }

    fn route_change(&self, change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
        match self.scope {
            CdcFanoutScope::All => route_change(change),
            CdcFanoutScope::WatchlistNotification => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::WatchlistNotification)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::SearchFilterPercolator => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::SearchFilterPercolator)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::SearchFilterMatchNotification => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::SearchFilterMatches
                ) && change.operation == CdcOperation::Insert
                {
                    search_filter_match_created_job(change)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::SearchFilterProjection => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::SearchFilters
                ) {
                    search_filter_changed_job(change, change.operation)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingOpenSearch => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::ProductListingOpenSearch)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingEmbedding => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::ProductListingEmbed)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingContentAssessment => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| {
                            job.target_queue == WorkerQueue::ProductListingContentAssessment
                        })
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingTranslation => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::ProductListingTranslate)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingRawNormalization => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingRawRevisions
                ) && change.operation == CdcOperation::Insert
                {
                    product_listing_raw_revision_job(change)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::NotificationDelivery => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::NotificationDeliveries
                ) && change.operation == CdcOperation::Insert
                {
                    notification_delivery_created_job(change)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
        }
    }
}

fn parse_cdc_batch(body: &str) -> Result<CdcBatch, serde_json::Error> {
    let value: Value = serde_json::from_str(body)?;

    if value
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("record-type"))
        .is_some()
    {
        return serde_json::from_value::<DmsKinesisRecord>(value)?.try_into();
    }

    if value.get("data").is_some() {
        return serde_json::from_value::<SequinWebhookBatch>(value).map(Into::into);
    }

    if value.get("metadata").is_some() && value.get("record").is_some() {
        return serde_json::from_value::<SequinWebhookMessage>(value).map(CdcBatch::from);
    }

    serde_json::from_value(value)
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

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct SequinWebhookBatch {
    data: Vec<SequinWebhookMessage>,
}

impl From<SequinWebhookBatch> for CdcBatch {
    fn from(batch: SequinWebhookBatch) -> Self {
        Self {
            delivery_id: None,
            source: Some("sequin-webhook".to_owned()),
            changes: batch.data.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct SequinWebhookMessage {
    record: Option<Value>,
    #[serde(default)]
    changes: Option<Map<String, Value>>,
    #[serde(deserialize_with = "deserialize_operation")]
    action: CdcOperation,
    metadata: SequinWebhookMetadata,
}

impl From<SequinWebhookMessage> for CdcBatch {
    fn from(message: SequinWebhookMessage) -> Self {
        Self {
            delivery_id: None,
            source: Some("sequin-webhook".to_owned()),
            changes: vec![message.into()],
        }
    }
}

impl From<SequinWebhookMessage> for CdcChange {
    fn from(message: SequinWebhookMessage) -> Self {
        let SequinWebhookMessage {
            record,
            changes,
            action,
            metadata,
        } = message;
        let changed_columns = changes
            .as_ref()
            .map(|changes| changes.keys().cloned().collect())
            .unwrap_or_default();
        let (record, old_record) = match action {
            CdcOperation::Delete => (None, record.or_else(|| changes.map(Value::Object))),
            CdcOperation::Insert | CdcOperation::Update => (record, changes.map(Value::Object)),
        };

        Self {
            schema: Some(metadata.table_schema),
            table: metadata.table_name,
            operation: action,
            primary_key: BTreeMap::new(),
            record,
            old_record,
            changed_columns,
            commit_lsn: metadata.commit_lsn.map(|value| match value {
                Value::String(value) => value,
                other => other.to_string(),
            }),
            commit_timestamp: metadata.commit_timestamp,
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct SequinWebhookMetadata {
    table_schema: String,
    table_name: String,
    #[serde(default)]
    commit_lsn: Option<Value>,
    #[serde(default)]
    commit_timestamp: Option<String>,
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
    #[error("worker queue closed before fanout: {0:?}")]
    QueueClosed(WorkerQueue),
}

impl From<mpsc::error::SendError<DomainJob>> for CdcFanoutError {
    fn from(error: mpsc::error::SendError<DomainJob>) -> Self {
        Self::QueueClosed(error.0.target_queue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueueConfig, in_memory_queue};

    fn product_event_change(event_type: &str, event_group: &str) -> CdcChange {
        let payload = match (event_type, event_group) {
            ("PRODUCT_LISTING_DISCOVERED", "DOMAIN") => serde_json::json!({
                "listingSourceId": "01900000-0000-7000-8000-000000000001",
                "sourceListingId": "fixture-source-id",
                "title": null,
                "description": null,
                "pricing": {
                    "price": null,
                    "priceEstimateMin": null,
                    "priceEstimateMax": null
                },
                "availability": null,
                "url": "https://example.test/product",
                "imageCount": 0,
                "auction": null
            }),
            ("PRODUCT_LISTING_CHANGED", "DOMAIN") => serde_json::json!({
                "availability": {"previous": null, "current": "AVAILABLE"}
            }),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT") => serde_json::json!({
                "sourceEventId": "01900000-0000-7000-8000-000000000005"
            }),
            ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT") => serde_json::json!({
                "sourceEventId": "01900000-0000-7000-8000-000000000005",
                "sourceLanguage": "de",
                "targetLanguages": ["en", "fr", "es", "it"]
            }),
            _ => serde_json::json!({}),
        };
        product_event_change_with_payload(event_type, event_group, payload)
    }

    fn product_event_change_with_payload(
        event_type: &str,
        event_group: &str,
        payload: Value,
    ) -> CdcChange {
        CdcChange {
            schema: Some("public".to_owned()),
            table: "product_listing_events".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "event_id": "01900000-0000-7000-8000-000000000004",
                "product_listing_id": "01900000-0000-7000-8000-000000000003",
                "event_type": event_type,
                "event_group": event_group,
                "event_type_schema_version": 1,
                "payload": payload,
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        }
    }

    fn raw_revision_change(operation: CdcOperation) -> CdcChange {
        CdcChange {
            schema: Some("public".to_owned()),
            table: "product_listing_raw_revisions".to_owned(),
            operation,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "product_listing_raw_stream_id": "01900000-0000-7000-8000-000000000001",
                "product_listing_raw_revision_id": "01900000-0000-7000-8000-000000000002",
                "revision": 3,
                "source_payload": {"mustNotEnterQueue": true},
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        }
    }

    #[test]
    fn should_route_raw_revision_insert_with_typed_wakeup_metadata_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = product_listing_raw_revision_job(&raw_revision_change(CdcOperation::Insert))?;
        let expected_stream_id = uuid::Uuid::parse_str("01900000-0000-7000-8000-000000000001")?;
        let expected_revision_id = uuid::Uuid::parse_str("01900000-0000-7000-8000-000000000002")?;

        assert!(matches!(
            jobs.as_slice(),
            [DomainJob {
                target_queue: WorkerQueue::ProductListingRawNormalization,
                idempotency_key,
                ordering_key,
                payload: DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
                    product_listing_raw_stream_id,
                    product_listing_raw_revision_id,
                    revision: 3,
                }),
            }]
                if idempotency_key.as_str() == "product-listing-raw-revision:prr_01j0000000e008000000000002"
                    && ordering_key.as_str() == "product-listing-raw-stream:prs_01j0000000e008000000000001"
                    && *product_listing_raw_stream_id.as_uuid() == expected_stream_id
                    && *product_listing_raw_revision_id.as_uuid() == expected_revision_id
        ));
        Ok(())
    }

    #[test]
    fn should_reject_malformed_raw_revision_wakeup_metadata() {
        for (field, value) in [
            (
                "product_listing_raw_stream_id",
                serde_json::json!("invalid"),
            ),
            (
                "product_listing_raw_revision_id",
                serde_json::json!("invalid"),
            ),
            ("revision", serde_json::json!(0)),
            ("revision", serde_json::json!(-1)),
        ] {
            let mut change = raw_revision_change(CdcOperation::Insert);
            if let Some(row) = change.record.as_mut() {
                row[field] = value;
            }

            assert!(product_listing_raw_revision_job(&change).is_err());
        }
    }

    #[test]
    fn should_reject_non_insert_or_other_table_for_raw_normalization_scope() {
        let registry = WorkerQueueRegistry::new();
        let fanout = CdcFanout::product_listing_raw_normalization(registry);

        assert!(
            fanout
                .route_change(&raw_revision_change(CdcOperation::Update))
                .is_err()
        );
        assert!(
            fanout
                .route_change(&product_event_change(
                    "PRODUCT_LISTING_DISCOVERED",
                    "DOMAIN"
                ))
                .is_err()
        );
    }

    #[test]
    fn should_route_discovered_events_with_tagged_main_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        for price in [
            serde_json::json!({"type": "MONETARY", "amount": 10_000, "currency": "EUR"}),
            serde_json::json!({"type": "ON_REQUEST"}),
        ] {
            let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
            if let Some(pricing) = change
                .record
                .as_mut()
                .and_then(|record| record.get_mut("payload"))
                .and_then(|payload| payload.get_mut("pricing"))
            {
                pricing["price"] = price;
            }

            let jobs = route_change(&change)?;
            assert_eq!(5, jobs.len());
        }
        Ok(())
    }

    #[test]
    fn should_reject_untagged_discovered_main_price_before_fanout() {
        let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(pricing) = change
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            .and_then(|payload| payload.get_mut("pricing"))
        {
            pricing["price"] = serde_json::json!({"amount": 10_000, "currency": "EUR"});
        }

        assert!(route_change(&change).is_err());
    }

    #[test]
    fn should_route_discovered_event_to_projection_percolator_assessment_embedding_and_translation()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&product_event_change(
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
        ))?;

        assert_eq!(5, jobs.len());
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingOpenSearch)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::SearchFilterPercolator)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingContentAssessment)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingEmbed)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingTranslate)
        );
        assert!(
            jobs.iter().all(|job| job.idempotency_key.as_str()
                == "product-event:evt_01j0000000e008000000000004")
        );
        assert!(
            jobs.iter()
                .all(|job| job.ordering_key.as_str() == "product:pl_01j0000000e008000000000003")
        );
        let expected_event_id =
            EventId::try_from(Uuid::parse_str("01900000-0000-7000-8000-000000000004")?)?;
        let expected_product_listing_id =
            ProductListingId::try_from(Uuid::parse_str("01900000-0000-7000-8000-000000000003")?)?;
        assert!(jobs.iter().all(|job| {
            matches!(
                &job.payload,
                DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                    event_id,
                    product_listing_id,
                    ..
                }) if *event_id == expected_event_id && *product_listing_id == expected_product_listing_id
            )
        }));
        Ok(())
    }

    #[test]
    fn should_route_all_supported_events_to_projection_and_percolator()
    -> Result<(), Box<dyn std::error::Error>> {
        for (event_type, event_group) in [
            ("PRODUCT_LISTING_DISCOVERED", "DOMAIN"),
            ("PRODUCT_LISTING_CHANGED", "DOMAIN"),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT"),
            ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT"),
        ] {
            let jobs = route_change(&product_event_change(event_type, event_group))?;

            assert!(
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingOpenSearch),
                "{event_type}"
            );
            assert!(
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::SearchFilterPercolator),
                "{event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_route_content_assessment_and_translation_only_for_discovered_event()
    -> Result<(), Box<dyn std::error::Error>> {
        for (event_type, event_group, expected) in [
            ("PRODUCT_LISTING_DISCOVERED", "DOMAIN", true),
            ("PRODUCT_LISTING_CHANGED", "DOMAIN", false),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT", false),
            ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT", false),
        ] {
            let jobs = route_change(&product_event_change(event_type, event_group))?;

            assert_eq!(
                expected,
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingContentAssessment),
                "assessment {event_type}"
            );
            assert_eq!(
                expected,
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingTranslate),
                "translation {event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_route_embedding_for_discovered_and_image_changed_events()
    -> Result<(), Box<dyn std::error::Error>> {
        for (event_type, event_group, payload, expected) in [
            (
                "PRODUCT_LISTING_DISCOVERED",
                "DOMAIN",
                serde_json::json!({
                    "listingSourceId": "01900000-0000-7000-8000-000000000001",
                    "sourceListingId": "fixture-source-id",
                    "title": null,
                    "description": null,
                    "pricing": {
                        "price": null,
                        "priceEstimateMin": null,
                        "priceEstimateMax": null
                    },
                    "availability": null,
                    "url": "https://example.test/product",
                    "imageCount": 0,
                    "auction": null
                }),
                true,
            ),
            (
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"images": {"previousCount": 1, "currentCount": 2}}),
                true,
            ),
            (
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
                false,
            ),
            (
                "ENRICHMENT_EMBEDDED",
                "ENRICHMENT",
                serde_json::json!({"sourceEventId": "01900000-0000-7000-8000-000000000005"}),
                false,
            ),
        ] {
            let jobs = route_change(&product_event_change_with_payload(
                event_type,
                event_group,
                payload,
            ))?;

            assert_eq!(
                expected,
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingEmbed),
                "embedding {event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_route_changed_event_to_one_watchlist_job_when_main_price_or_availability_changed()
    -> Result<(), Box<dyn std::error::Error>> {
        for payload in [
            serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": null,
                        "current": {"type": "MONETARY", "amount": 900, "currency": "USD"}
                    }
                }
            }),
            serde_json::json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
            serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": {"type": "MONETARY", "amount": 1200, "currency": "USD"},
                        "current": {"type": "MONETARY", "amount": 900, "currency": "USD"}
                    }
                },
                "availability": {"previous": "IN_STOCK", "current": "SOLD_OUT"}
            }),
        ] {
            let jobs = route_change(&product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                payload,
            ))?;

            assert_eq!(
                1,
                jobs.iter()
                    .filter(|job| job.target_queue == WorkerQueue::WatchlistNotification)
                    .count()
            );
        }
        Ok(())
    }

    #[test]
    fn should_fan_out_embedding_and_watchlist_for_combined_image_and_price_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({
                "images": {"previousCount": 1, "currentCount": 2},
                "pricing": {
                    "price": {
                        "previous": {"type": "MONETARY", "amount": 1200, "currency": "USD"},
                        "current": {"type": "MONETARY", "amount": 900, "currency": "USD"}
                    }
                }
            }),
        ))?;

        assert_eq!(
            1,
            jobs.iter()
                .filter(|job| job.target_queue == WorkerQueue::ProductListingEmbed)
                .count()
        );
        assert_eq!(
            1,
            jobs.iter()
                .filter(|job| job.target_queue == WorkerQueue::WatchlistNotification)
                .count()
        );
        Ok(())
    }

    #[test]
    fn should_not_route_estimate_only_changed_event_to_watchlist_notifications()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({
                "pricing": {
                    "priceEstimateMin": {
                        "previous": {"amount": 1200, "currency": "USD"},
                        "current": {"amount": 900, "currency": "USD"}
                    },
                    "priceEstimateMax": {
                        "previous": null,
                        "current": {"amount": 1500, "currency": "USD"}
                    }
                }
            }),
        ))?;

        assert!(
            jobs.iter()
                .all(|job| job.target_queue != WorkerQueue::WatchlistNotification)
        );
        Ok(())
    }

    #[test]
    fn should_reject_product_listing_event_with_malformed_ids() {
        for (field, value, expected) in [
            ("event_id", "not-a-uuid", CdcRouteError::InvalidEventId),
            (
                "product_listing_id",
                "not-a-uuid",
                CdcRouteError::InvalidProductListingId,
            ),
        ] {
            let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
            if let Some(row) = change.record.as_mut() {
                row[field] = serde_json::json!(value);
            }

            assert!(matches!(route_change(&change), Err(error) if error == expected));
        }
    }

    #[test]
    fn should_reject_typeids_noncanonical_uuid_and_non_v7_uuid_at_cdc_boundary() {
        for value in [
            "evt_01h455vb4pex5vy7enb1p677vn",
            "01890A5D-AC96-774B-BF1D-D5586C639F75",
            "550e8400-e29b-41d4-a716-446655440000",
        ] {
            let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
            if let Some(row) = change.record.as_mut() {
                row["event_id"] = serde_json::json!(value);
            }
            assert!(matches!(
                route_change(&change),
                Err(CdcRouteError::InvalidEventId)
            ));
        }

        let mut raw = raw_revision_change(CdcOperation::Insert);
        if let Some(row) = raw.record.as_mut() {
            row["product_listing_raw_stream_id"] =
                serde_json::json!("prs_01h455vb4pex5vy7enb1p677vn");
        }
        assert!(product_listing_raw_revision_job(&raw).is_err());

        let mut search_filter = CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "usr_01h455vb4pex5vy7enb1p677vn",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "version": 1
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };
        assert!(search_filter_changed_job(&search_filter, CdcOperation::Insert).is_err());
        if let Some(row) = search_filter.record.as_mut() {
            row["user_id"] = serde_json::json!("01900000-0000-7000-8000-000000000001");
            row["user_search_filter_id"] = serde_json::json!("sf_01h455vb4pex5vy7enb1p677vn");
        }
        assert!(search_filter_changed_job(&search_filter, CdcOperation::Insert).is_err());

        let search_filter_match = CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filter_matches".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "product_listing_id": "01900000-0000-7000-8000-000000000003",
                "origin_event_id": "evt_01h455vb4pex5vy7enb1p677vn"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };
        assert!(search_filter_match_created_job(&search_filter_match).is_err());

        let delivery = CdcChange {
            schema: Some("public".to_owned()),
            table: "notification_deliveries".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "notification_delivery_id": "nd_01h455vb4pex5vy7enb1p677vn"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };
        assert!(notification_delivery_created_job(&delivery).is_err());
    }

    #[test]
    fn should_reject_typeids_in_internal_product_listing_event_json() {
        let mut discovered = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = discovered
            .record
            .as_mut()
            .and_then(|row| row.get_mut("payload"))
        {
            payload["listingSourceId"] = serde_json::json!("ls_01h455vb4pex5vy7enb1p677vn");
        }
        assert!(route_change(&discovered).is_err());

        let embedded = product_event_change_with_payload(
            "ENRICHMENT_EMBEDDED",
            "ENRICHMENT",
            serde_json::json!({"sourceEventId": "evt_01h455vb4pex5vy7enb1p677vn"}),
        );
        assert!(route_change(&embedded).is_err());

        let sale = product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({
                "saleObservation": {
                    "transition": "OBSERVED",
                    "observation": {
                        "observedAt": "1970-01-01T00:00:00Z",
                        "fxRateId": "fx_01h455vb4pex5vy7enb1p677vn"
                    }
                }
            }),
        );
        assert!(route_change(&sale).is_err());
    }

    #[test]
    fn should_reject_product_listing_event_with_missing_ids() {
        for field in ["event_id", "product_listing_id"] {
            let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
            if let Some(row) = change.record.as_mut()
                && let Some(row) = row.as_object_mut()
            {
                let _ = row.remove(field);
            }

            assert!(matches!(
                route_change(&change),
                Err(CdcRouteError::MissingColumn(missing)) if missing == field
            ));
        }
    }

    #[test]
    fn should_reject_unknown_or_incompatible_product_listing_event_codes() {
        for (event_type, event_group) in [
            ("PRODUCT_LISTING_UNKNOWN", "DOMAIN"),
            ("PRODUCT_LISTING_CHANGED", "ENRICHMENT"),
            ("ENRICHMENT_EMBEDDED", "DOMAIN"),
        ] {
            let result = route_change(&product_event_change(event_type, event_group));

            assert!(matches!(
                result,
                Err(CdcRouteError::UnsupportedProductListingEvent { .. })
            ));
        }
    }

    #[test]
    fn should_require_supported_product_listing_event_schema_version() {
        let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(row) = change.record.as_mut() {
            row["event_type_schema_version"] = serde_json::json!(2);
        }

        assert!(matches!(
            route_change(&change),
            Err(CdcRouteError::UnsupportedProductListingEventSchemaVersion { schema_version: 2 })
        ));
    }

    #[test]
    fn should_accept_tagged_main_price_changes_for_watchlist_routing()
    -> Result<(), Box<dyn std::error::Error>> {
        for price in [
            serde_json::json!({"previous": null, "current": {"type": "MONETARY", "amount": 900, "currency": "USD"}}),
            serde_json::json!({"previous": {"type": "MONETARY", "amount": 900, "currency": "USD"}, "current": {"type": "ON_REQUEST"}}),
            serde_json::json!({"previous": {"type": "ON_REQUEST"}, "current": null}),
        ] {
            let jobs = route_change(&product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"pricing": {"price": price}}),
            ))?;
            assert!(
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::WatchlistNotification)
            );
        }
        Ok(())
    }

    #[test]
    fn should_reject_untagged_or_invalid_tagged_main_price_changes_before_fanout() {
        for price in [
            serde_json::json!({"previous": null, "current": {"amount": 900, "currency": "USD"}}),
            serde_json::json!({"previous": null, "current": {"type": "UNKNOWN"}}),
            serde_json::json!({"previous": null, "current": {"type": "MONETARY", "amount": 900}}),
            serde_json::json!({"previous": null, "current": {"type": "ON_REQUEST", "amount": 900}}),
        ] {
            assert!(
                route_change(&product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({"pricing": {"price": price}}),
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn should_reject_malformed_changed_event_shapes_before_fanout() {
        for payload in [
            serde_json::json!({"pricing": {"price": {}}}),
            serde_json::json!({"availability": {}}),
            serde_json::json!({"images": {}}),
            serde_json::json!({}),
            serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": null,
                        "current": null
                    }
                }
            }),
        ] {
            assert!(
                route_change(&product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    payload,
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn should_reject_product_listing_v1_negative_contract_matrix() {
        let mut unknown_discovery = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = unknown_discovery
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
        {
            payload["unexpected"] = serde_json::json!(true);
        }

        let mut omitted_title = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = omitted_title
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            && let Some(payload) = payload.as_object_mut()
        {
            payload.remove("title");
        }

        let mut omitted_price = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = omitted_price
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            .and_then(|payload| payload.get_mut("pricing"))
            && let Some(pricing) = payload.as_object_mut()
        {
            pricing.remove("price");
        }

        let mut omitted_auction_membership =
            product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = omitted_auction_membership
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
        {
            payload["auction"] = serde_json::json!({
                "auctionId": null,
                "lotNumber": null,
                "cataloguePosition": null,
                "lotBiddingOpensAt": null,
                "lotScheduledClosesAt": null,
                "lotReportedClosedAt": null
            });
        }
        if let Some(payload) = omitted_auction_membership
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            .and_then(|payload| payload.get_mut("auction"))
            && let Some(auction) = payload.as_object_mut()
        {
            auction.remove("auctionId");
        }

        let cases = vec![
            ("unknown discovery field", unknown_discovery),
            ("omitted nullable discovery field", omitted_title),
            ("omitted pricing field", omitted_price),
            ("omitted auction field", omitted_auction_membership),
            (
                "unknown localized field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_DISCOVERED",
                    "DOMAIN",
                    serde_json::json!({
                        "listingSourceId": "01900000-0000-7000-8000-000000000001",
                        "sourceListingId": "fixture-source-id",
                        "title": {"language": "en", "text": "Title", "unexpected": true},
                        "description": null,
                        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
                        "availability": null,
                        "url": "https://example.test/product",
                        "imageCount": 0,
                        "auction": null
                    }),
                ),
            ),
            (
                "noncanonical discovery URL",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_DISCOVERED",
                    "DOMAIN",
                    serde_json::json!({
                        "listingSourceId": "01900000-0000-7000-8000-000000000001",
                        "sourceListingId": "fixture-source-id",
                        "title": null,
                        "description": null,
                        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
                        "availability": null,
                        "url": "https://example.com:443/product",
                        "imageCount": 0,
                        "auction": null
                    }),
                ),
            ),
            (
                "unparsable changed URL",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "url": {"previous": "not a URL", "current": "https://example.com/new"}
                    }),
                ),
            ),
            (
                "noncanonical changed URL",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "url": {"previous": "https://example.com/old", "current": "https://example.com:443/new"}
                    }),
                ),
            ),
            (
                "unknown image field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({"images": {"previousCount": 1, "currentCount": 2, "unexpected": true}}),
                ),
            ),
            (
                "unknown value change field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "availability": {"previous": null, "current": "AVAILABLE", "unexpected": true}
                    }),
                ),
            ),
            (
                "unknown price field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "pricing": {
                            "price": {
                                "previous": {"type": "MONETARY", "amount": 1, "currency": "EUR", "unexpected": true},
                                "current": null
                            }
                        }
                    }),
                ),
            ),
            (
                "invalid lot auction timestamp",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "auction": {
                            "previous": {
                                "auctionId": null,
                                "lotNumber": null,
                                "cataloguePosition": null,
                                "lotBiddingOpensAt": "not a timestamp",
                                "lotScheduledClosesAt": null,
                                "lotReportedClosedAt": null
                            },
                            "current": null
                        }
                    }),
                ),
            ),
            (
                "date-only lot auction timestamp",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "auction": {
                            "previous": {
                                "auctionId": null,
                                "lotNumber": null,
                                "cataloguePosition": null,
                                "lotBiddingOpensAt": "2025-01-01",
                                "lotScheduledClosesAt": null,
                                "lotReportedClosedAt": null
                            },
                            "current": null
                        }
                    }),
                ),
            ),
            (
                "lot bidding opens after scheduled close",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "auction": {
                            "previous": {
                                "auctionId": null,
                                "lotNumber": null,
                                "cataloguePosition": null,
                                "lotBiddingOpensAt": "2025-01-02T00:00:00Z",
                                "lotScheduledClosesAt": "2025-01-01T00:00:00Z",
                                "lotReportedClosedAt": null
                            },
                            "current": null
                        }
                    }),
                ),
            ),
            (
                "invalid sale observation timestamp",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "saleObservation": {
                            "transition": "OBSERVED",
                            "observation": {
                                "observedAt": "yesterday",
                                "fxRateId": "01900000-0000-7000-8000-000000000001"
                            }
                        }
                    }),
                ),
            ),
            (
                "invalid sale observation FX ID",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "saleObservation": {
                            "transition": "OBSERVED",
                            "observation": {
                                "observedAt": "1970-01-01T00:00:00Z",
                                "fxRateId": "not-a-uuid"
                            }
                        }
                    }),
                ),
            ),
            (
                "withdrawal with current availability",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "availability": {"previous": null, "current": "AVAILABLE"},
                        "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": null}
                    }),
                ),
            ),
            (
                "restoration with previous availability",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "availability": {"previous": "AVAILABLE", "current": null},
                        "lifecycle": {"transition": "RESTORED"}
                    }),
                ),
            ),
        ];

        for (name, change) in cases {
            assert!(route_change(&change).is_err(), "{name}");
        }
    }

    #[test]
    fn should_reject_retired_auction_start_end_event_shape() {
        let change = product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({
                "auction": {
                    "previous": {"start": null, "end": null},
                    "current": null
                }
            }),
        );

        assert!(route_change(&change).is_err());
    }

    #[test]
    fn should_accept_product_listing_v1_positive_contract_matrix() {
        let cases = [
            product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN"),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"images": {"previousCount": 2, "currentCount": 2}}),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "auction": {
                        "previous": null,
                        "current": {
                            "auctionId": null,
                            "lotNumber": "42A",
                            "cataloguePosition": 7,
                            "lotBiddingOpensAt": "2026-10-01T08:00:00Z",
                            "lotScheduledClosesAt": "2026-10-05T18:32:00Z",
                            "lotReportedClosedAt": null
                        }
                    }
                }),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "pricing": {"price": {"previous": null, "current": {"type": "MONETARY", "amount": 10, "currency": "EUR"}}},
                    "availability": {"previous": null, "current": "AVAILABLE"},
                    "images": {"previousCount": 1, "currentCount": 1}
                }),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "availability": {"previous": "AVAILABLE", "current": null},
                    "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": "AVAILABLE"}
                }),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "availability": {"previous": null, "current": "AVAILABLE"},
                    "lifecycle": {"transition": "RESTORED"}
                }),
            ),
            product_event_change("ENRICHMENT_EMBEDDED", "ENRICHMENT"),
            product_event_change("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT"),
        ];

        for change in cases {
            assert!(route_change(&change).is_ok());
        }
    }

    #[test]
    fn should_accept_same_count_image_replacement_for_embedding_routing() {
        let result = route_change(&product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({"images": {"previousCount": 2, "currentCount": 2}}),
        ));
        assert!(result.is_ok());
        assert!(
            result
                .unwrap_or_default()
                .iter()
                .any(|job| { job.target_queue == WorkerQueue::ProductListingEmbed })
        );
    }

    #[test]
    fn should_ignore_product_listings_table_to_avoid_double_fire()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "product_listings".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({ "product_listing_id": "p1" })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[test]
    fn should_route_search_filter_when_relevant_columns_change()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "version": 3,
            })),
            old_record: None,
            changed_columns: vec!["search".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::SearchFilterOpenSearch, jobs[0].target_queue);
        assert_eq!(
            "search-filter:sf_01j0000000e008000000000006:3:update",
            jobs[0].idempotency_key.as_str()
        );
        assert!(matches!(
            &jobs[0].payload,
            DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
                user_id,
                user_search_filter_id,
                ..
            }) if user_id.to_string() == "usr_01j0000000e008000000000001"
                && user_search_filter_id.to_string() == "sf_01j0000000e008000000000006"
        ));
        Ok(())
    }

    #[test]
    fn should_route_search_filter_when_any_persisted_column_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "version": 3,
            })),
            old_record: None,
            changed_columns: vec!["name".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::SearchFilterOpenSearch, jobs[0].target_queue);
        Ok(())
    }

    #[test]
    fn should_reject_search_filter_change_without_source_version() {
        let result = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
            })),
            old_record: None,
            changed_columns: vec!["name".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        });

        assert!(matches!(
            result,
            Err(CdcRouteError::MissingColumn("version"))
        ));
    }

    #[test]
    fn should_route_every_notification_delivery_insert_to_delivery_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "notification_deliveries".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "notification_delivery_id": "01900000-0000-7000-8000-000000000007",
                "channel": "EMAIL"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::NotificationDelivery, jobs[0].target_queue);
        assert_eq!(
            "notification-delivery:nd_01j0000000e008000000000007",
            jobs[0].idempotency_key.as_str()
        );
        assert_eq!(
            "notification-delivery:nd_01j0000000e008000000000007",
            jobs[0].ordering_key.as_str()
        );
        assert!(matches!(
            &jobs[0].payload,
            DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
                notification_delivery_id,
            }) if notification_delivery_id.to_string() == "nd_01j0000000e008000000000007"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_only_delivery_inserts_for_notification_delivery_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(1))?;
        let fanout = CdcFanout::notification_delivery(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::NotificationDelivery, sender),
        );
        let change = CdcChange {
            schema: Some("public".to_owned()),
            table: "notification_deliveries".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "notification_delivery_id": "01900000-0000-7000-8000-000000000007"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };

        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-notification".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![change],
                })
                .await?
        );
        assert_eq!(
            Some(WorkerQueue::NotificationDelivery),
            receiver.recv().await.map(|job| job.target_queue)
        );
        Ok(())
    }

    #[test]
    fn should_route_search_filter_match_insert_to_notification_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filter_matches".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "product_listing_id": "01900000-0000-7000-8000-000000000003",
                "origin_event_id": "01900000-0000-7000-8000-000000000004"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(
            WorkerQueue::SearchFilterMatchNotification,
            jobs[0].target_queue
        );
        assert_eq!(
            "search-filter-match:usr_01j0000000e008000000000001:sf_01j0000000e008000000000006:pl_01j0000000e008000000000003:evt_01j0000000e008000000000004",
            jobs[0].idempotency_key.as_str()
        );
        assert!(matches!(
            &jobs[0].payload,
            DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
                user_id,
                user_search_filter_id,
                product_listing_id,
                origin_event_id,
            }) if user_id.to_string() == "usr_01j0000000e008000000000001"
                && user_search_filter_id.to_string() == "sf_01j0000000e008000000000006"
                && product_listing_id.to_string() == "pl_01j0000000e008000000000003"
                && origin_event_id.to_string() == "evt_01j0000000e008000000000004"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_only_match_inserts_for_search_filter_match_notification_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(1))?;
        let fanout = CdcFanout::search_filter_match_notification(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::SearchFilterMatchNotification, sender),
        );
        let change = CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filter_matches".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "product_listing_id": "01900000-0000-7000-8000-000000000003",
                "origin_event_id": "01900000-0000-7000-8000-000000000004"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };

        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-match".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![change],
                })
                .await?
        );
        assert_eq!(
            Some(WorkerQueue::SearchFilterMatchNotification),
            receiver.recv().await.map(|job| job.target_queue)
        );
        Ok(())
    }

    #[test]
    fn should_not_route_user_tier_change_without_a_production_consumer()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "users".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "tier": "PREMIUM",
                "version": 4,
            })),
            old_record: Some(serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "tier": "FREE",
                "version": 3,
            })),
            changed_columns: vec!["tier".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[test]
    fn should_ignore_unknown_table() -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "future_table".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({ "id": "1" })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_register_exactly_ten_scopes_and_publish_the_maximum_discovery_fanout() {
        use strum::IntoEnumIterator;
        let scopes: Vec<_> = crate::WorkerScope::iter().collect();
        let queues: std::collections::HashSet<_> = WorkerQueue::ALL.into_iter().collect();
        assert_eq!(10, scopes.len());
        assert_eq!(10, queues.len());
        assert!(!queues.contains(&WorkerQueue::UserTierEnforcement));
        assert_eq!(
            queues,
            scopes.iter().map(|scope| scope.consumer_queue()).collect()
        );
        let (registry, mut receivers) =
            WorkerQueueRegistry::with_all_queues(QueueConfig::new(MAX_CDC_CHANGES)).unwrap();
        let fanout = CdcFanout::new(registry);
        let change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        let expected = route_change(&change).unwrap();
        assert_eq!(5, expected.len());
        let batch = CdcBatch {
            delivery_id: None,
            source: None,
            changes: vec![change; MAX_CDC_CHANGES],
        };
        assert_eq!(MAX_CDC_JOBS, fanout.ingest_batch(&batch).await.unwrap());
        for queue in WorkerQueue::ALL {
            let mut receiver = receivers.take(queue).unwrap();
            let mut count = 0;
            while let Ok(job) = receiver.receiver.try_recv() {
                assert_eq!(queue, job.target_queue);
                assert!(job.validate().is_ok());
                count += 1;
            }
            assert_eq!(
                if expected.iter().any(|job| job.target_queue == queue) {
                    MAX_CDC_CHANGES
                } else {
                    0
                },
                count
            );
        }
    }

    #[tokio::test]
    async fn should_ack_after_all_jobs_are_enqueued() -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (percolator_sender, mut percolator_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (embed_sender, mut embed_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (assessment_sender, mut assessment_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (translation_sender, mut translation_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let registry = WorkerQueueRegistry::new()
            .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
            .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender)
            .with_queue(WorkerQueue::ProductListingEmbed, embed_sender)
            .with_queue(
                WorkerQueue::ProductListingContentAssessment,
                assessment_sender,
            )
            .with_queue(WorkerQueue::ProductListingTranslate, translation_sender);
        let fanout = CdcFanout::new(registry);
        let batch = CdcBatch {
            delivery_id: Some("delivery-1".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let enqueued = fanout.ingest_batch(&batch).await?;

        assert_eq!(5, enqueued);
        assert!(product_receiver.recv().await.is_some());
        assert!(percolator_receiver.recv().await.is_some());
        assert!(embed_receiver.recv().await.is_some());
        assert!(assessment_receiver.recv().await.is_some());
        assert!(translation_receiver.recv().await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_publish_nothing_when_any_destination_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let (percolator_sender, mut percolator_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let fanout = CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
                .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender),
        );
        let batch = CdcBatch {
            delivery_id: Some("delivery-partial-fanout".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let result = fanout.ingest_batch(&batch).await;

        assert!(matches!(
            result,
            Err(CdcIngestError::Fanout(CdcFanoutError::MissingQueue(
                WorkerQueue::ProductListingContentAssessment
            )))
        ));
        assert!(matches!(
            product_receiver.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            percolator_receiver.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_all_discovery_jobs_after_missing_destinations_are_repaired()
    -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) = in_memory_queue(QueueConfig::new(2))?;
        let (percolator_sender, mut percolator_receiver) = in_memory_queue(QueueConfig::new(2))?;
        let (assessment_sender, mut assessment_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let (embed_sender, mut embed_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let (translation_sender, mut translation_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let partial_fanout = CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(
                    WorkerQueue::ProductListingOpenSearch,
                    product_sender.clone(),
                )
                .with_queue(
                    WorkerQueue::SearchFilterPercolator,
                    percolator_sender.clone(),
                ),
        );
        let retry_fanout = CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
                .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender)
                .with_queue(
                    WorkerQueue::ProductListingContentAssessment,
                    assessment_sender,
                )
                .with_queue(WorkerQueue::ProductListingEmbed, embed_sender)
                .with_queue(WorkerQueue::ProductListingTranslate, translation_sender),
        );
        let batch = CdcBatch {
            delivery_id: Some("delivery-redelivery".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let partial_result = partial_fanout.ingest_batch(&batch).await;

        assert!(matches!(
            partial_result,
            Err(CdcIngestError::Fanout(CdcFanoutError::MissingQueue(
                WorkerQueue::ProductListingContentAssessment
            )))
        ));
        assert_eq!(5, retry_fanout.ingest_batch(&batch).await?);

        assert_eq!(
            Some(WorkerQueue::ProductListingOpenSearch),
            product_receiver.recv().await.map(|job| job.target_queue)
        );
        assert_eq!(
            Some(WorkerQueue::SearchFilterPercolator),
            percolator_receiver.recv().await.map(|job| job.target_queue)
        );
        assert!(matches!(
            product_receiver.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            percolator_receiver.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            Some(WorkerQueue::ProductListingContentAssessment),
            assessment_receiver.recv().await.map(|job| job.target_queue)
        );
        assert_eq!(
            Some(WorkerQueue::ProductListingEmbed),
            embed_receiver.recv().await.map(|job| job.target_queue)
        );
        assert_eq!(
            Some(WorkerQueue::ProductListingTranslate),
            translation_receiver
                .recv()
                .await
                .map(|job| job.target_queue)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_only_domain_or_enrichment_product_listing_events_for_percolator_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(2))?;
        let fanout = CdcFanout::search_filter_percolator(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterPercolator, sender),
        );

        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-domain".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![product_event_change("PRODUCT_LISTING_CHANGED", "DOMAIN")],
                })
                .await?
        );
        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-enrichment".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![product_event_change("ENRICHMENT_EMBEDDED", "ENRICHMENT")],
                })
                .await?
        );

        for _ in 0..2 {
            assert_eq!(
                Some(WorkerQueue::SearchFilterPercolator),
                receiver.recv().await.map(|job| job.target_queue)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_non_search_filter_change_for_search_filter_projection_worker() {
        let (sender, _receiver) = in_memory_queue(QueueConfig::new(1))
            .unwrap_or_else(|error| panic!("queue setup failed: {error}"));
        let fanout = CdcFanout::search_filter_projection(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterOpenSearch, sender),
        );
        let batch = CdcBatch {
            delivery_id: Some("delivery-1".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let result = fanout.ingest_batch(&batch).await;

        assert!(matches!(
            result,
            Err(CdcIngestError::Route(CdcRouteError::UnsupportedTableForWorker(table)))
                if table == "product_listing_events"
        ));
    }

    #[tokio::test]
    async fn should_fail_fanout_when_target_queue_is_missing() {
        let fanout = CdcFanout::new(WorkerQueueRegistry::new());
        let batch = CdcBatch {
            delivery_id: Some("delivery-1".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let result = fanout.ingest_batch(&batch).await;

        assert!(matches!(
            result,
            Err(CdcIngestError::Fanout(CdcFanoutError::MissingQueue(
                WorkerQueue::ProductListingOpenSearch
            )))
        ));
    }

    #[test]
    fn should_parse_sequin_like_json_with_aliases() -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "id": "delivery-1",
                "source": "postgres",
                "events": [
                    {
                        "table_schema": "public",
                        "relation": "product_listing_events",
                        "op": "insert",
                        "keys": { "event_id": "01900000-0000-7000-8000-000000000004" },
                        "new": {
                            "event_id": "01900000-0000-7000-8000-000000000004",
                            "product_listing_id": "01900000-0000-7000-8000-000000000003",
                            "event_type": "PRODUCT_LISTING_DISCOVERED",
                            "event_group": "DOMAIN",
                            "event_type_schema_version": 1
                        }
                    }
                ]
            }"#,
        )?;

        assert_eq!(Some("delivery-1".to_owned()), batch.delivery_id);
        assert_eq!(1, batch.changes.len());
        assert_eq!(CdcOperation::Insert, batch.changes[0].operation);
        Ok(())
    }

    #[test]
    fn should_parse_real_sequin_webhook_message() -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "record": {
                    "event_id": "01900000-0000-7000-8000-000000000004",
                    "product_listing_id": "01900000-0000-7000-8000-000000000003",
                    "event_type": "PRODUCT_LISTING_DISCOVERED",
                    "event_group": "DOMAIN",
                    "event_type_schema_version": 1
                },
                "changes": null,
                "action": "insert",
                "metadata": {
                    "table_schema": "public",
                    "table_name": "product_listing_events",
                    "commit_timestamp": "2026-07-25T12:00:00Z",
                    "commit_lsn": 123456789
                }
            }"#,
        )?;

        assert_eq!(Some("sequin-webhook".to_owned()), batch.source);
        assert_eq!(1, batch.changes.len());
        assert_eq!("product_listing_events", batch.changes[0].table);
        assert_eq!(CdcOperation::Insert, batch.changes[0].operation);
        assert_eq!(Some("123456789".to_owned()), batch.changes[0].commit_lsn);
        Ok(())
    }

    #[test]
    fn should_parse_real_sequin_delete_webhook_message_as_old_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "record": {
                    "user_id": "01900000-0000-7000-8000-000000000001",
                    "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                    "version": 2
                },
                "changes": null,
                "action": "delete",
                "metadata": {
                    "table_schema": "public",
                    "table_name": "search_filters"
                }
            }"#,
        )?;

        assert_eq!(CdcOperation::Delete, batch.changes[0].operation);
        assert!(batch.changes[0].record.is_none());
        assert_eq!(
            Some(&serde_json::json!({
                "user_id": "01900000-0000-7000-8000-000000000001",
                "user_search_filter_id": "01900000-0000-7000-8000-000000000006",
                "version": 2
            })),
            batch.changes[0].old_record.as_ref()
        );
        Ok(())
    }

    #[test]
    fn should_parse_real_sequin_webhook_batch() -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "data": [
                    {
                        "record": {
                            "user_id": "01900000-0000-7000-8000-000000000001",
                            "tier": "PREMIUM",
                            "version": 2
                        },
                        "changes": { "tier": "FREE" },
                        "action": "update",
                        "metadata": {
                            "table_schema": "public",
                            "table_name": "users"
                        }
                    }
                ]
            }"#,
        )?;

        assert_eq!(1, batch.changes.len());
        assert_eq!("users", batch.changes[0].table);
        assert_eq!(vec!["tier".to_owned()], batch.changes[0].changed_columns);
        Ok(())
    }

    #[tokio::test]
    async fn legacy_webhook_still_accepts_dms_shaped_input() {
        // Compatibility for the existing native HTTP ingress, not evidence of Kinesis capture.
        let body = r#"{"data":{"notification_delivery_id":"01900000-0000-7000-8000-000000000007"},"metadata":{"record-type":"data","operation":"insert","schema-name":"public","table-name":"notification_deliveries"}}"#;
        let batch = parse_cdc_batch(body).expect("existing native ingress accepts DMS-shaped JSON");
        assert_eq!(Some(DMS_KINESIS_SOURCE), batch.source.as_deref());
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(1)).unwrap();
        let fanout = CdcFanout::notification_delivery(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::NotificationDelivery, sender),
        );
        assert_eq!(1, fanout.ingest_json(body).await.unwrap());
        assert_eq!(
            Some(WorkerQueue::NotificationDelivery),
            receiver.recv().await.map(|job| job.target_queue)
        );
    }
}
