use application::error::BoxError;
use domain_primitives::event_id::EventId;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterMonthlyMatchQuotaReadError {
    #[error("monthly search filter match quota read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterMonthlyMatchQuotaReader: Send {
    /// Returns the stable one-based notification-selection rank for this user's event.
    /// One lowest-filter match is counted for each origin event, not every persisted match row.
    async fn notification_selection_rank_for_user_in_month(
        &mut self,
        user_id: UserId,
        matched_at: OffsetDateTime,
        origin_event_id: EventId,
    ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError>;
}

pub trait SearchFilterMonthlyMatchQuotaReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx;
}
