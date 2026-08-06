use common::postgres::SqlxTransaction;
use common::user_id::UserId;
use search_filter_service::ports::{
    SearchFilterQuotaReadError, SearchFilterQuotaReader, SearchFilterQuotaReaderFactory,
};

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterQuotaReaderFactory;

struct SqlxSearchFilterQuotaReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterQuotaReaderFactory<SqlxTransaction> for SqlxSearchFilterQuotaReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterQuotaReader + 'tx {
        SqlxSearchFilterQuotaReader { tx }
    }
}

#[async_trait::async_trait]
impl SearchFilterQuotaReader for SqlxSearchFilterQuotaReader<'_> {
    async fn count_active_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<usize, SearchFilterQuotaReadError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM search_filters WHERE user_id=$1 AND state='Active'",
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_one(self.tx.connection())
        .await
        .map_err(|_| SearchFilterQuotaReadError::ReadFailed)?;
        usize::try_from(count).map_err(|_| SearchFilterQuotaReadError::ReadFailed)
    }
}
