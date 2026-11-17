//! One module per consumed scope. Each maps a claimed job to its use case and the use case's
//! outcome to a queue disposition, exactly as that scope's Lambda adapter acknowledged, retained
//! or parked its SQS record, and composes the scope's production use case.

pub mod geo_resolution;
pub mod listing_geography;
pub mod listing_shipping;
pub mod marketing_consent_sync;
pub mod notification_delivery;
pub mod product_content_assessment;
pub mod product_embedding;
pub mod product_listing_ingestion;
pub mod product_listing_normalization;
pub mod product_listing_opensearch;
pub mod product_translation;
pub mod search_filter_match_notification;
pub mod user_identity_deletion;
pub mod watchlist_notification;

#[cfg(test)]
pub(crate) mod test_support {
    use platform_postgres_queue::{GroupKey, JobId};
    use platform_worker::Job;

    /// A first delivery of `payload` on a standard scope.
    pub(crate) fn claimed_job(payload: &str) -> Job {
        Job::new(
            JobId::from_uuid(uuid::Uuid::now_v7()),
            1,
            None,
            payload.to_owned(),
        )
    }

    /// Delivery `attempt` of `payload` in FIFO group `group`.
    pub(crate) fn fifo_job(payload: &str, group: &str, attempt: u32) -> Job {
        Job::new(
            JobId::from_uuid(uuid::Uuid::now_v7()),
            attempt,
            Some(GroupKey::new(group).expect("valid group key")),
            payload.to_owned(),
        )
    }
}
