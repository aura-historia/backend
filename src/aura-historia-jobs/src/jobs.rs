//! Compact jobs. PostgreSQL and target-side guards, not transport memory, own idempotency.
use crate::WorkerScope;
use domain_primitives::event_id::EventId;
use geo_service::resolution::ResolutionId;
use notification_core::notification_delivery_id::NotificationDeliveryId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_raw_id::{
    ProductListingRawRevisionId, ProductListingRawStreamId,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use strum::IntoEnumIterator;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::user_id::UserId;

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
    ProductListingRawNormalization,
    WatchlistNotification,
    SearchFilterMatchNotification,
    ProductListingContentAssessment,
    ProductListingEmbed,
    ProductListingTranslate,
    /// Legacy in-memory API only. No production scope or wire representation.
    UserTierEnforcement,
    NotificationDelivery,
    MarketingConsentSync,
    GeoResolution,
    ListingGeography,
    ListingShipping,
    UserIdentityDeletion,
}

impl WorkerQueue {
    pub const ALL: [Self; 13] = [
        Self::ProductListingOpenSearch,
        Self::ProductListingRawNormalization,
        Self::WatchlistNotification,
        Self::SearchFilterMatchNotification,
        Self::ProductListingContentAssessment,
        Self::ProductListingEmbed,
        Self::ProductListingTranslate,
        Self::NotificationDelivery,
        Self::MarketingConsentSync,
        Self::GeoResolution,
        Self::ListingGeography,
        Self::ListingShipping,
        Self::UserIdentityDeletion,
    ];

    pub fn scope(self) -> Option<WorkerScope> {
        WorkerScope::iter().find(|scope| scope.consumer_queue() == self)
    }
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
    ProductListingRawRevision(ProductListingRawRevisionJob),
    SearchFilterMatchCreated(SearchFilterMatchCreatedJob),
    UserTierChanged(UserTierChangedJob),
    NotificationDeliveryCreated(NotificationDeliveryCreatedJob),
    MarketingConsentSyncIntentCreated(MarketingConsentSyncIntentCreatedJob),
    GeoResolution(GeoResolutionJob),
    ListingGeography(ListingGeographyJob),
    ListingShipping(ListingShippingJob),
    UserIdentityDeletion(UserIdentityDeletionJob),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIdentityDeletionJob {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingGeographyJob {
    pub work_id: product_listing_service::geography::GeographyWorkId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingShippingJob {
    pub work_id: product_listing_service::shipping_projection::ShippingWorkId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeoResolutionJob {
    pub resolution_id: ResolutionId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingEventJob {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

/// Compact wake-up metadata; immutable revision content is reread from PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawRevisionJob {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub product_listing_raw_revision_id: ProductListingRawRevisionId,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilterMatchCreatedJob {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub product_listing_id: ProductListingId,
    pub origin_event_id: EventId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTierChangedJob {
    pub user_id: UserId,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDeliveryCreatedJob {
    pub notification_delivery_id: NotificationDeliveryId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketingConsentSyncIntentCreatedJob {
    pub marketing_consent_sync_intent_id: MarketingConsentSyncIntentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("job identifiers, version, scope, or logical keys are invalid")]
pub struct InvalidJob;

impl DomainJob {
    pub fn validate(&self) -> Result<WorkerScope, InvalidJob> {
        let scope = self.target_queue.scope().ok_or(InvalidJob)?;
        let permitted = match &self.payload {
            DomainJobPayload::ProductListingEvent(_) => matches!(
                scope,
                WorkerScope::ProductListingOpenSearch
                    | WorkerScope::WatchlistNotification
                    | WorkerScope::ProductListingContentAssessment
                    | WorkerScope::ProductListingEmbedding
                    | WorkerScope::ProductListingTranslation
            ),
            DomainJobPayload::ProductListingRawRevision(_) => {
                scope == WorkerScope::ProductListingRawNormalization
            }
            DomainJobPayload::SearchFilterMatchCreated(_) => {
                scope == WorkerScope::SearchFilterMatchNotification
            }
            DomainJobPayload::NotificationDeliveryCreated(_) => {
                scope == WorkerScope::NotificationDelivery
            }
            DomainJobPayload::MarketingConsentSyncIntentCreated(_) => {
                scope == WorkerScope::MarketingConsentSync
            }
            DomainJobPayload::UserTierChanged(_) => false,
            DomainJobPayload::ListingGeography(_) => scope == WorkerScope::ListingGeography,
            DomainJobPayload::ListingShipping(_) => scope == WorkerScope::ListingShipping,
            DomainJobPayload::UserIdentityDeletion(_) => scope == WorkerScope::UserIdentityDeletion,
            DomainJobPayload::GeoResolution(_) => scope == WorkerScope::GeoResolution,
        };
        let keys_match = match &self.payload {
            DomainJobPayload::MarketingConsentSyncIntentCreated(job) => {
                self.idempotency_key.as_str()
                    == format!("marketing-consent:{}", job.marketing_consent_sync_intent_id)
                    && valid_marketing_recipient_group(self.ordering_key.as_str())
            }
            _ => {
                let (idempotency, ordering) = self.payload.logical_keys()?;
                self.idempotency_key.as_str() == idempotency
                    && self.ordering_key.as_str() == ordering
            }
        };
        if !permitted || !keys_match {
            return Err(InvalidJob);
        }
        Ok(scope)
    }
}

impl DomainJobPayload {
    pub fn logical_keys(&self) -> Result<(String, String), InvalidJob> {
        Ok(match self {
            Self::ListingGeography(job) => {
                if job.generation == 0 || job.generation > i64::MAX as u64 {
                    return Err(InvalidJob);
                }
                (
                    format!("listing-geography:{}:{}", job.work_id, job.generation),
                    format!("listing-geography:{}", job.work_id),
                )
            }
            Self::UserIdentityDeletion(job) => {
                let key = format!("user-identity-deletion:{}", job.user_id);
                (key.clone(), key)
            }
            Self::ListingShipping(job) => {
                if job.generation == 0 || job.generation > i64::MAX as u64 {
                    return Err(InvalidJob);
                }
                (
                    format!("listing-shipping:{}:{}", job.work_id, job.generation),
                    format!("listing-shipping:{}", job.work_id),
                )
            }
            Self::GeoResolution(job) => {
                if job.generation == 0 || job.generation > i64::MAX as u64 {
                    return Err(InvalidJob);
                }
                (
                    format!("geo-resolution:{}:{}", job.resolution_id, job.generation),
                    format!("geo-resolution:{}", job.resolution_id),
                )
            }
            Self::ProductListingEvent(event) => (
                format!("product-event:{}", event.event_id),
                format!("product:{}", event.product_listing_id),
            ),
            Self::ProductListingRawRevision(revision) => {
                if revision.revision == 0 || revision.revision > i64::MAX as u64 {
                    return Err(InvalidJob);
                }
                (
                    format!(
                        "product-listing-raw-revision:{}",
                        revision.product_listing_raw_revision_id
                    ),
                    format!(
                        "product-listing-raw-stream:{}",
                        revision.product_listing_raw_stream_id
                    ),
                )
            }
            Self::SearchFilterMatchCreated(change) => (
                format!(
                    "search-filter-match:{}:{}:{}:{}",
                    change.user_id,
                    change.user_search_filter_id,
                    change.product_listing_id,
                    change.origin_event_id
                ),
                format!("user:{}", change.user_id),
            ),
            Self::NotificationDeliveryCreated(delivery) => {
                let key = format!(
                    "notification-delivery:{}",
                    delivery.notification_delivery_id
                );
                (key.clone(), key)
            }
            Self::MarketingConsentSyncIntentCreated(_) => return Err(InvalidJob),
            Self::UserTierChanged(_) => return Err(InvalidJob),
        })
    }
}

fn valid_marketing_recipient_group(value: &str) -> bool {
    let Some(recipient_key) = value.strip_prefix("marketing-email:") else {
        return false;
    };
    recipient_key.len() == 64
        && recipient_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
