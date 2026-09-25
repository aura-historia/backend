//! Compatibility imports for worker CDC and consumers; the contract lives in aura-historia-jobs.
use crate::cdc::CdcOperation;
use aura_historia_jobs::jobs::SearchFilterOperation;

pub use aura_historia_jobs::jobs::{
    IdempotencyKey, InvalidJob, NotificationDeliveryCreatedJob, OrderingKey,
    ProductListingEventJob, ProductListingRawRevisionJob, SearchFilterMatchCreatedJob,
    UserTierChangedJob, WorkerQueue,
};

pub type DomainJob = aura_historia_jobs::jobs::DomainJob<CdcOperation>;
pub type DomainJobPayload = aura_historia_jobs::jobs::DomainJobPayload<CdcOperation>;
pub type SearchFilterChangedJob = aura_historia_jobs::jobs::SearchFilterChangedJob<CdcOperation>;

impl From<CdcOperation> for SearchFilterOperation {
    fn from(operation: CdcOperation) -> Self {
        match operation {
            CdcOperation::Insert => Self::Insert,
            CdcOperation::Update => Self::Update,
            CdcOperation::Delete => Self::Delete,
        }
    }
}

impl From<SearchFilterOperation> for CdcOperation {
    fn from(operation: SearchFilterOperation) -> Self {
        match operation {
            SearchFilterOperation::Insert => Self::Insert,
            SearchFilterOperation::Update => Self::Update,
            SearchFilterOperation::Delete => Self::Delete,
        }
    }
}
