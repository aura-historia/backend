use crate::mapping::{MATCH_COLUMNS, MatchRow, user_search_filter_uuid};
use platform_postgres::SqlxTransaction;
use product_listing_core::product_id::ProductId;
use search_filter_core::SearchFilterProductMatch;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    PersistedSearchFilterMatch, SearchFilterMatchRepository, SearchFilterMatchRepositoryError,
    SearchFilterMatchRepositoryFactory,
};
use sqlx::{Postgres, QueryBuilder};
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
    ) -> Result<Option<PersistedSearchFilterMatch>, SearchFilterMatchRepositoryError> {
        let filter_id = user_search_filter_uuid(filter_id)
            .map_err(|_| SearchFilterMatchRepositoryError::LookupFailed)?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(MATCH_COLUMNS)
            .push(" FROM search_filter_matches WHERE user_search_filter_id=$1 AND product_id=$2");
        query
            .build_query_as::<MatchRow>()
            .bind(filter_id)
            .bind(uuid::Uuid::from(product_id))
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|_| SearchFilterMatchRepositoryError::LookupFailed)?
            .map(PersistedSearchFilterMatch::try_from)
            .transpose()
            .map_err(|_| SearchFilterMatchRepositoryError::InvalidPersistedState)
    }
    async fn insert(
        &mut self,
        v: &SearchFilterProductMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError> {
        let id = user_search_filter_uuid(v.user_search_filter_id)
            .map_err(|_| SearchFilterMatchRepositoryError::InsertFailed)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO search_filter_matches (user_id,user_search_filter_id,product_id,origin_event_id,price_valuation_basis,price_fx_rate_id,user_search_filter_name,enhanced_match_reason,feedback) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING ",
        );
        query.push(MATCH_COLUMNS);
        let row = query
            .build_query_as::<MatchRow>()
            .bind(uuid::Uuid::from(v.user_id))
            .bind(id)
            .bind(uuid::Uuid::from(v.product_id))
            .bind(uuid::Uuid::from(v.origin_event_id))
            .bind(
                v.price_match_valuation
                    .map(|valuation| valuation.basis.as_str()),
            )
            .bind(
                v.price_match_valuation
                    .map(|valuation| uuid::Uuid::from(valuation.fx_rate_id)),
            )
            .bind(v.user_search_filter_name.as_ref().map(AsRef::as_ref))
            .bind(v.enhanced_match_reason.as_ref().map(AsRef::as_ref))
            .bind(v.feedback)
            .fetch_one(self.tx.connection())
            .await
            .map_err(|_| SearchFilterMatchRepositoryError::InsertFailed)?;
        row.try_into()
            .map_err(|_| SearchFilterMatchRepositoryError::InvalidPersistedState)
    }
    async fn update(
        &mut self,
        v: &SearchFilterProductMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError> {
        let id = user_search_filter_uuid(v.user_search_filter_id)
            .map_err(|_| SearchFilterMatchRepositoryError::UpdateFailed)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "UPDATE search_filter_matches SET user_search_filter_name=$3,enhanced_match_reason=$4,feedback=$5,updated=now() WHERE user_search_filter_id=$1 AND product_id=$2 RETURNING ",
        );
        query.push(MATCH_COLUMNS);
        let row = query
            .build_query_as::<MatchRow>()
            .bind(id)
            .bind(uuid::Uuid::from(v.product_id))
            .bind(v.user_search_filter_name.as_ref().map(AsRef::as_ref))
            .bind(v.enhanced_match_reason.as_ref().map(AsRef::as_ref))
            .bind(v.feedback)
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|_| SearchFilterMatchRepositoryError::UpdateFailed)?
            .ok_or(SearchFilterMatchRepositoryError::UpdateFailed)?;
        row.try_into()
            .map_err(|_| SearchFilterMatchRepositoryError::InvalidPersistedState)
    }
}
