use common::error::boxed::BoxError;
use common::user_id::UserId;
use time::OffsetDateTime;

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
    async fn count_for_user_in_month(
        &mut self,
        user_id: UserId,
        occurred_at: OffsetDateTime,
    ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError>;
}

pub trait SearchFilterMonthlyMatchQuotaReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx;
}
