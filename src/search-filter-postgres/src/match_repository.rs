use crate::mapping::{MatchRow, user_search_filter_uuid};
use common::postgres::SqlxTransaction;
use common::product_id::ProductId;
use common::user_search_filter_id::UserSearchFilterId;
use search_filter_core::SearchFilterProductMatch;
use search_filter_service::ports::{
    SearchFilterMatchRepository, SearchFilterMatchRepositoryError,
    SearchFilterMatchRepositoryFactory,
};

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMatchRepositoryFactory;

struct SqlxSearchFilterMatchRepository<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMatchRepositoryFactory<SqlxTransaction>
    for SqlxSearchFilterMatchRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMatchRepository + 'tx {
        SqlxSearchFilterMatchRepository { tx }
    }
}

#[async_trait::async_trait]
impl SearchFilterMatchRepository for SqlxSearchFilterMatchRepository<'_> {
    async fn find_by_filter_and_product(
        &mut self,
        filter_id: UserSearchFilterId,
        product_id: ProductId,
    ) -> Result<Option<SearchFilterProductMatch>, SearchFilterMatchRepositoryError> {
        let row = sqlx::query_as::<_, MatchRow>(
            "SELECT user_id, user_search_filter_id, product_id, origin_event_id, user_search_filter_name, enhanced_match_reason, feedback, created, updated \
             FROM search_filter_matches WHERE user_search_filter_id=$1 AND product_id=$2",
        )
        .bind(user_search_filter_uuid(filter_id))
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|_| SearchFilterMatchRepositoryError::LookupFailed)?;
        Ok(row.map(Into::into))
    }

    async fn insert(
        &mut self,
        product_match: &SearchFilterProductMatch,
    ) -> Result<(), SearchFilterMatchRepositoryError> {
        sqlx::query(
            "INSERT INTO search_filter_matches \
             (user_id, user_search_filter_id, product_id, origin_event_id, user_search_filter_name, enhanced_match_reason, feedback, created, updated) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(uuid::Uuid::from(product_match.user_id))
        .bind(user_search_filter_uuid(product_match.user_search_filter_id))
        .bind(uuid::Uuid::from(product_match.product_id))
        .bind(uuid::Uuid::from(product_match.origin_event_id))
        .bind(product_match.user_search_filter_name.as_ref().map(|v| v.as_ref()))
        .bind(product_match.enhanced_match_reason.as_ref().map(|v| v.as_ref()))
        .bind(product_match.feedback)
        .bind(product_match.created)
        .bind(product_match.updated)
        .execute(self.tx.connection())
        .await
        .map_err(|_| SearchFilterMatchRepositoryError::InsertFailed)?;
        Ok(())
    }

    async fn update(
        &mut self,
        product_match: &SearchFilterProductMatch,
    ) -> Result<(), SearchFilterMatchRepositoryError> {
        sqlx::query(
            "UPDATE search_filter_matches SET user_search_filter_name=$3, enhanced_match_reason=$4, feedback=$5, updated=$6 \
             WHERE user_search_filter_id=$1 AND product_id=$2",
        )
        .bind(user_search_filter_uuid(product_match.user_search_filter_id))
        .bind(uuid::Uuid::from(product_match.product_id))
        .bind(product_match.user_search_filter_name.as_ref().map(|v| v.as_ref()))
        .bind(product_match.enhanced_match_reason.as_ref().map(|v| v.as_ref()))
        .bind(product_match.feedback)
        .bind(product_match.updated)
        .execute(self.tx.connection())
        .await
        .map_err(|_| SearchFilterMatchRepositoryError::UpdateFailed)?;
        Ok(())
    }
}
