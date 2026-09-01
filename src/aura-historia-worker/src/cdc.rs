use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::{
    InMemoryQueueReceiver, InMemoryQueueSender, QueueConfig, QueueConfigError, in_memory_queue,
};

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct CdcBatch {
    #[serde(default, alias = "id", alias = "webhook_id")]
    pub delivery_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, alias = "events", alias = "records")]
    pub changes: Vec<CdcChange>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct CdcChange {
    #[serde(default, alias = "table_schema")]
    pub schema: Option<String>,
    #[serde(alias = "relation")]
    pub table: String,
    #[serde(
        alias = "op",
        alias = "action",
        deserialize_with = "deserialize_operation"
    )]
    pub operation: CdcOperation,
    #[serde(default, alias = "keys")]
    pub primary_key: BTreeMap<String, Value>,
    #[serde(default, alias = "new", alias = "new_record")]
    pub record: Option<Value>,
    #[serde(default, rename = "old", alias = "old_record", alias = "previous")]
    pub old_record: Option<Value>,
    #[serde(default, alias = "changed")]
    pub changed_columns: Vec<String>,
    #[serde(default)]
    pub commit_lsn: Option<String>,
    #[serde(default)]
    pub commit_timestamp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcOperation {
    Insert,
    Update,
    Delete,
}

impl WorkerQueue {
    pub const ALL: [Self; 10] = [
        Self::ProductListingOpenSearch,
        Self::WatchlistNotification,
        Self::SearchFilterPercolator,
        Self::SearchFilterMatchNotification,
        Self::ProductListingContentAssessment,
        Self::ProductListingEmbed,
        Self::ProductListingTranslate,
        Self::SearchFilterOpenSearch,
        Self::UserTierEnforcement,
        Self::NotificationDelivery,
    ];
}

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

fn deserialize_operation<'de, D>(deserializer: D) -> Result<CdcOperation, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    CdcOperation::from_str(&value).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainJob {
    pub target_queue: WorkerQueue,
    pub idempotency_key: IdempotencyKey,
    pub ordering_key: OrderingKey,
    pub payload: DomainJobPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerQueue {
    ProductListingOpenSearch,
    WatchlistNotification,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    ProductListingContentAssessment,
    ProductListingEmbed,
    ProductListingTranslate,
    SearchFilterOpenSearch,
    UserTierEnforcement,
    NotificationDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderingKey(String);

impl OrderingKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainJobPayload {
    ProductListingEvent(ProductListingEventJob),
    SearchFilterChanged(SearchFilterChangedJob),
    SearchFilterMatchCreated(SearchFilterMatchCreatedJob),
    UserTierChanged(UserTierChangedJob),
    NotificationDeliveryCreated(NotificationDeliveryCreatedJob),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingEventJob {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
    pub event_type: String,
    pub event_group: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilterChangedJob {
    pub user_id: String,
    pub user_search_filter_id: String,
    pub version: i64,
    pub operation: CdcOperation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilterMatchCreatedJob {
    pub user_id: String,
    pub user_search_filter_id: String,
    pub product_listing_id: String,
    pub origin_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTierChangedJob {
    pub user_id: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDeliveryCreatedJob {
    pub notification_delivery_id: String,
}

#[derive(Debug, Default, Clone)]
pub struct WorkerQueueRegistry {
    queues: HashMap<WorkerQueue, InMemoryQueueSender<DomainJob>>,
}

impl WorkerQueueRegistry {
    pub fn new() -> Self {
        Self::default()
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
        self.queues.insert(queue, sender);
        self
    }

    async fn enqueue(&self, job: DomainJob) -> Result<(), CdcFanoutError> {
        let Some(sender) = self.queues.get(&job.target_queue) else {
            return Err(CdcFanoutError::MissingQueue(job.target_queue));
        };
        sender
            .enqueue(job)
            .await
            .map_err(|error| CdcFanoutError::QueueClosed(error.0.target_queue))
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
    NotificationDelivery,
}

impl CdcFanout {
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

    pub fn notification_delivery(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::NotificationDelivery,
        }
    }

    pub async fn ingest_json(&self, body: &str) -> Result<usize, CdcIngestError> {
        let batch = parse_cdc_batch(body).map_err(CdcIngestError::InvalidJson)?;
        self.ingest_batch(&batch).await
    }

    pub async fn ingest_batch(&self, batch: &CdcBatch) -> Result<usize, CdcIngestError> {
        let mut enqueued = 0;

        for change in &batch.changes {
            for job in self.route_change(change)? {
                self.registry.enqueue(job).await?;
                enqueued += 1;
            }
        }

        debug!(
            changes = batch.changes.len(),
            enqueued, "CDC batch fanned out"
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

    if value.get("data").is_some() {
        return serde_json::from_value::<SequinWebhookBatch>(value).map(Into::into);
    }

    if value.get("metadata").is_some() && value.get("record").is_some() {
        return serde_json::from_value::<SequinWebhookMessage>(value).map(CdcBatch::from);
    }

    serde_json::from_value(value)
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

pub fn route_change(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let table = CdcTable::from(change.table.as_str());
    match (table, change.operation) {
        (CdcTable::ProductListingEvents, CdcOperation::Insert) => product_event_jobs(change),
        (CdcTable::ProductListingEvents, _) => Ok(Vec::new()),
        (CdcTable::SearchFilters, operation) => search_filter_changed_job(change, operation),
        (CdcTable::SearchFilterMatches, CdcOperation::Insert) => {
            search_filter_match_created_job(change)
        }
        (CdcTable::SearchFilterMatches, _) => Ok(Vec::new()),
        (CdcTable::Users, CdcOperation::Update) => user_tier_changed_job(change),
        (CdcTable::Users, _) => Ok(Vec::new()),
        (CdcTable::NotificationDeliveries, CdcOperation::Insert) => {
            notification_delivery_created_job(change)
        }
        (CdcTable::NotificationDeliveries, _) => Ok(Vec::new()),
        (CdcTable::Unknown(table), _) => {
            warn!(%table, operation = %change.operation, "ignoring unregistered CDC table");
            Ok(Vec::new())
        }
        (CdcTable::ProductListings | CdcTable::ProductListingWatchlist, _) => Ok(Vec::new()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CdcTable {
    ProductListingEvents,
    ProductListings,
    SearchFilters,
    SearchFilterMatches,
    Users,
    ProductListingWatchlist,
    NotificationDeliveries,
    Unknown(String),
}

impl From<&str> for CdcTable {
    fn from(value: &str) -> Self {
        match value {
            "product_listing_events" => Self::ProductListingEvents,
            "product_listings" => Self::ProductListings,
            "search_filters" => Self::SearchFilters,
            "search_filter_matches" => Self::SearchFilterMatches,
            "users" => Self::Users,
            "product_listing_watchlist" => Self::ProductListingWatchlist,
            "notification_deliveries" => Self::NotificationDeliveries,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

const PRODUCT_LISTING_EVENT_SCHEMA_VERSION: i64 = 1;

fn product_event_jobs(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = required_row(change)?;
    let event_id = EventId::try_from(required_string(row, "event_id")?.as_str())
        .map_err(|_| CdcRouteError::InvalidEventId)?;
    let product_listing_id =
        ProductListingId::try_from(required_string(row, "product_listing_id")?.as_str())
            .map_err(|_| CdcRouteError::InvalidProductListingId)?;
    let event_type = required_string(row, "event_type")?;
    let event_group = required_string(row, "event_group")?;
    let schema_version = required_integer(row, "event_type_schema_version")?;
    validate_product_listing_event_contract(&event_type, &event_group, schema_version)?;

    let base_job = ProductListingEventJob {
        event_id,
        product_listing_id,
        event_type: event_type.clone(),
        event_group,
    };
    let idempotency_key = IdempotencyKey::new(format!("product-event:{event_id}"));
    let ordering_key = OrderingKey::new(format!("product:{product_listing_id}"));

    let mut jobs = vec![domain_job(
        WorkerQueue::ProductListingOpenSearch,
        idempotency_key.clone(),
        ordering_key.clone(),
        DomainJobPayload::ProductListingEvent(base_job.clone()),
    )];

    jobs.push(domain_job(
        WorkerQueue::SearchFilterPercolator,
        idempotency_key.clone(),
        ordering_key.clone(),
        DomainJobPayload::ProductListingEvent(base_job.clone()),
    ));

    if event_type == "PRODUCT_LISTING_CHANGED" {
        if changed_payload_requires_watchlist(row)? {
            jobs.push(domain_job(
                WorkerQueue::WatchlistNotification,
                idempotency_key.clone(),
                ordering_key.clone(),
                DomainJobPayload::ProductListingEvent(base_job.clone()),
            ));
        }

        if changed_payload_requires_embedding(row)? {
            jobs.push(domain_job(
                WorkerQueue::ProductListingEmbed,
                idempotency_key.clone(),
                ordering_key.clone(),
                DomainJobPayload::ProductListingEvent(base_job.clone()),
            ));
        }
    }

    if event_type == "PRODUCT_LISTING_DISCOVERED" {
        jobs.push(domain_job(
            WorkerQueue::ProductListingContentAssessment,
            idempotency_key.clone(),
            ordering_key.clone(),
            DomainJobPayload::ProductListingEvent(base_job.clone()),
        ));
        jobs.push(domain_job(
            WorkerQueue::ProductListingEmbed,
            idempotency_key.clone(),
            ordering_key.clone(),
            DomainJobPayload::ProductListingEvent(base_job.clone()),
        ));
        jobs.push(domain_job(
            WorkerQueue::ProductListingTranslate,
            idempotency_key,
            ordering_key,
            DomainJobPayload::ProductListingEvent(base_job),
        ));
    }

    Ok(jobs)
}

fn validate_product_listing_event_contract(
    event_type: &str,
    event_group: &str,
    schema_version: i64,
) -> Result<(), CdcRouteError> {
    if schema_version != PRODUCT_LISTING_EVENT_SCHEMA_VERSION {
        return Err(CdcRouteError::UnsupportedProductListingEventSchemaVersion { schema_version });
    }

    match (event_type, event_group) {
        ("PRODUCT_LISTING_DISCOVERED" | "PRODUCT_LISTING_CHANGED", "DOMAIN")
        | ("ENRICHMENT_EMBEDDED" | "ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT") => Ok(()),
        _ => Err(CdcRouteError::UnsupportedProductListingEvent {
            event_type: event_type.to_owned(),
            event_group: event_group.to_owned(),
        }),
    }
}

fn changed_payload_requires_watchlist(row: &Value) -> Result<bool, CdcRouteError> {
    let payload = changed_payload(row)?;
    let price_changed = payload
        .get("pricing")
        .and_then(Value::as_object)
        .is_some_and(|pricing| pricing.contains_key("price"));

    Ok(price_changed || payload.contains_key("availability"))
}

fn changed_payload_requires_embedding(row: &Value) -> Result<bool, CdcRouteError> {
    Ok(changed_payload(row)?.contains_key("images"))
}

fn changed_payload(row: &Value) -> Result<&Map<String, Value>, CdcRouteError> {
    row.get("payload")
        .ok_or(CdcRouteError::MissingColumn("payload"))?
        .as_object()
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn search_filter_changed_job(
    change: &CdcChange,
    operation: CdcOperation,
) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = row_for_operation(change)?;
    let user_search_filter_id = required_string(row, "user_search_filter_id")?;
    let user_id = required_string(row, "user_id")?;
    let version = required_integer(row, "version")?;

    Ok(vec![domain_job(
        WorkerQueue::SearchFilterOpenSearch,
        IdempotencyKey::new(format!(
            "search-filter:{user_search_filter_id}:{version}:{operation}"
        )),
        OrderingKey::new(format!("search-filter:{user_search_filter_id}")),
        DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
            user_id,
            user_search_filter_id,
            version,
            operation,
        }),
    )])
}

fn search_filter_match_created_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = required_row(change)?;
    let user_id = required_string(row, "user_id")?;
    let user_search_filter_id = required_string(row, "user_search_filter_id")?;
    let product_listing_id = required_string(row, "product_listing_id")?;
    let origin_event_id = required_string(row, "origin_event_id")?;

    Ok(vec![domain_job(
        WorkerQueue::SearchFilterMatchNotification,
        IdempotencyKey::new(format!(
            "search-filter-match:{user_id}:{user_search_filter_id}:{product_listing_id}:{origin_event_id}"
        )),
        OrderingKey::new(format!("user:{user_id}")),
        DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
            user_id,
            user_search_filter_id,
            product_listing_id,
            origin_event_id,
        }),
    )])
}

fn notification_delivery_created_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = required_row(change)?;
    let notification_delivery_id = required_string(row, "notification_delivery_id")?;

    Ok(vec![domain_job(
        WorkerQueue::NotificationDelivery,
        IdempotencyKey::new(format!("notification-delivery:{notification_delivery_id}")),
        OrderingKey::new(format!("notification-delivery:{notification_delivery_id}")),
        DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
            notification_delivery_id,
        }),
    )])
}

fn user_tier_changed_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    if !has_tier_change(change) {
        return Ok(Vec::new());
    }

    let row = required_row(change)?;
    let user_id = required_string(row, "user_id")?;
    let version = integer_field(row, "version").unwrap_or(0);

    Ok(vec![domain_job(
        WorkerQueue::UserTierEnforcement,
        IdempotencyKey::new(format!("user-tier:{user_id}:{version}")),
        OrderingKey::new(format!("user:{user_id}")),
        DomainJobPayload::UserTierChanged(UserTierChangedJob { user_id, version }),
    )])
}

fn domain_job(
    target_queue: WorkerQueue,
    idempotency_key: IdempotencyKey,
    ordering_key: OrderingKey,
    payload: DomainJobPayload,
) -> DomainJob {
    DomainJob {
        target_queue,
        idempotency_key,
        ordering_key,
        payload,
    }
}

fn row_for_operation(change: &CdcChange) -> Result<&Value, CdcRouteError> {
    match change.operation {
        CdcOperation::Delete => change.old_record.as_ref().ok_or(CdcRouteError::MissingRow),
        CdcOperation::Insert | CdcOperation::Update => required_row(change),
    }
}

fn required_row(change: &CdcChange) -> Result<&Value, CdcRouteError> {
    change.record.as_ref().ok_or(CdcRouteError::MissingRow)
}

fn required_string(row: &Value, field: &'static str) -> Result<String, CdcRouteError> {
    string_field(row, field).ok_or(CdcRouteError::MissingColumn(field))
}

fn string_field(row: &Value, field: &str) -> Option<String> {
    let value = row.get(field)?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn integer_field(row: &Value, field: &str) -> Option<i64> {
    row.get(field)?.as_i64()
}

fn required_integer(row: &Value, field: &'static str) -> Result<i64, CdcRouteError> {
    integer_field(row, field).ok_or(CdcRouteError::MissingColumn(field))
}

fn has_tier_change(change: &CdcChange) -> bool {
    if !change.changed_columns.is_empty() {
        return change.changed_columns.iter().any(|column| column == "tier");
    }

    let Some(new) = &change.record else {
        return false;
    };
    let Some(old) = &change.old_record else {
        return true;
    };

    string_field(new, "tier") != string_field(old, "tier")
}

#[derive(thiserror::Error, Debug)]
pub enum CdcIngestError {
    #[error("invalid CDC JSON")]
    InvalidJson(#[source] serde_json::Error),
    #[error(transparent)]
    Route(#[from] CdcRouteError),
    #[error(transparent)]
    Fanout(#[from] CdcFanoutError),
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CdcRouteError {
    #[error("CDC table is not configured for this worker: {0}")]
    UnsupportedTableForWorker(String),
    #[error("CDC change has unsupported product listing event {event_type} in group {event_group}")]
    UnsupportedProductListingEvent {
        event_type: String,
        event_group: String,
    },
    #[error("CDC change has unsupported product listing event schema version {schema_version}")]
    UnsupportedProductListingEventSchemaVersion { schema_version: i64 },
    #[error("CDC change has an invalid product listing event ID")]
    InvalidEventId,
    #[error("CDC change has an invalid product listing ID")]
    InvalidProductListingId,
    #[error("CDC change has a product listing event payload that is not an object")]
    InvalidProductListingEventPayload,
    #[error("CDC change missing row data")]
    MissingRow,
    #[error("CDC row missing required column {0}")]
    MissingColumn(&'static str),
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CdcFanoutError {
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
        product_event_change_with_payload(event_type, event_group, serde_json::json!({}))
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
                "event_id": "40000000-0000-0000-0000-000000000001",
                "product_listing_id": "30000000-0000-0000-0000-000000000001",
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
        assert!(jobs.iter().all(|job| job.idempotency_key.as_str()
            == "product-event:40000000-0000-0000-0000-000000000001"));
        assert!(
            jobs.iter()
                .all(|job| job.ordering_key.as_str()
                    == "product:30000000-0000-0000-0000-000000000001")
        );
        let expected_event_id = EventId::try_from("40000000-0000-0000-0000-000000000001")?;
        let expected_product_listing_id =
            ProductListingId::try_from("30000000-0000-0000-0000-000000000001")?;
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
                serde_json::json!({}),
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
                serde_json::json!({}),
                false,
            ),
            (
                "ENRICHMENT_EMBEDDED",
                "ENRICHMENT",
                serde_json::json!({}),
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
            serde_json::json!({"pricing": {"price": {}}}),
            serde_json::json!({"availability": {}}),
            serde_json::json!({"pricing": {"price": {}}, "availability": {}}),
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
                "pricing": {"price": {}}
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
                    "priceEstimateMin": {},
                    "priceEstimateMax": {}
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
    fn should_reject_old_or_incompatible_product_listing_event_codes() {
        for (event_type, event_group) in [
            ("PRODUCT_LISTING_CREATED", "DOMAIN"),
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
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
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
            "search-filter:50000000-0000-0000-0000-000000000001:3:update",
            jobs[0].idempotency_key.as_str()
        );
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
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
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
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
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
                "notification_delivery_id": "60000000-0000-0000-0000-000000000001",
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
            "notification-delivery:60000000-0000-0000-0000-000000000001",
            jobs[0].idempotency_key.as_str()
        );
        assert_eq!(
            "notification-delivery:60000000-0000-0000-0000-000000000001",
            jobs[0].ordering_key.as_str()
        );
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
                "notification_delivery_id": "60000000-0000-0000-0000-000000000001"
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
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "product_listing_id": "30000000-0000-0000-0000-000000000001",
                "origin_event_id": "40000000-0000-0000-0000-000000000001"
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
            "search-filter-match:10000000-0000-0000-0000-000000000001:50000000-0000-0000-0000-000000000001:30000000-0000-0000-0000-000000000001:40000000-0000-0000-0000-000000000001",
            jobs[0].idempotency_key.as_str()
        );
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
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "product_listing_id": "30000000-0000-0000-0000-000000000001",
                "origin_event_id": "40000000-0000-0000-0000-000000000001"
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
    fn should_route_user_tier_change() -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "users".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "tier": "PREMIUM",
                "version": 4,
            })),
            old_record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "tier": "FREE",
                "version": 3,
            })),
            changed_columns: vec!["tier".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::UserTierEnforcement, jobs[0].target_queue);
        assert_eq!(
            "user-tier:10000000-0000-0000-0000-000000000001:4",
            jobs[0].idempotency_key.as_str()
        );
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
    async fn should_not_ack_partial_product_event_fanout_after_some_jobs_enqueue()
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
        assert!(product_receiver.recv().await.is_some());
        assert!(percolator_receiver.recv().await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_all_discovery_jobs_after_redelivery_of_partial_fanout()
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

        let first_product_job = product_receiver
            .recv()
            .await
            .ok_or("product queue stopped")?;
        let retried_product_job = product_receiver
            .recv()
            .await
            .ok_or("product queue stopped")?;
        let first_percolator_job = percolator_receiver
            .recv()
            .await
            .ok_or("percolator queue stopped")?;
        let retried_percolator_job = percolator_receiver
            .recv()
            .await
            .ok_or("percolator queue stopped")?;
        assert_eq!(first_product_job, retried_product_job);
        assert_eq!(first_percolator_job, retried_percolator_job);
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
                        "keys": { "event_id": "40000000-0000-0000-0000-000000000001" },
                        "new": {
                            "event_id": "40000000-0000-0000-0000-000000000001",
                            "product_listing_id": "30000000-0000-0000-0000-000000000001",
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
                    "event_id": "40000000-0000-0000-0000-000000000001",
                    "product_listing_id": "30000000-0000-0000-0000-000000000001",
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
                    "user_id": "10000000-0000-0000-0000-000000000001",
                    "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
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
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
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
                            "user_id": "10000000-0000-0000-0000-000000000001",
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
}
