use crate::mapping::{MATCH_COLUMNS, MatchRow};
use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::SearchFilterProductListingMatch;
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
        product_listing_id: ProductListingId,
    ) -> Result<Option<PersistedSearchFilterMatch>, SearchFilterMatchRepositoryError> {
        let filter_id = filter_id.into_uuid();
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(MATCH_COLUMNS).push(
            " FROM search_filter_matches WHERE user_search_filter_id=$1 AND product_listing_id=$2",
        );
        query
            .build_query_as::<MatchRow>()
            .bind(filter_id)
            .bind(product_listing_id.into_uuid())
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|source| SearchFilterMatchRepositoryError::LookupFailed {
                source: box_error(source),
            })?
            .map(PersistedSearchFilterMatch::try_from)
            .transpose()
            .map_err(
                |source| SearchFilterMatchRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                },
            )
    }
    async fn insert(
        &mut self,
        v: &SearchFilterProductListingMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError> {
        let id = v.user_search_filter_id.into_uuid();
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO search_filter_matches (user_id,user_search_filter_id,product_listing_id,origin_event_id,price_valuation_basis,price_fx_rate_id,user_search_filter_name,feedback,shipping_fx_rate_id,shipping_fx_captured_at,shipping_target_currency,shipping_maximum_minor) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING ",
        );
        query.push(MATCH_COLUMNS);
        let row = query
            .build_query_as::<MatchRow>()
            .bind(v.user_id.into_uuid())
            .bind(id)
            .bind(v.product_listing_id.into_uuid())
            .bind(v.origin_event_id.into_uuid())
            .bind(
                v.price_match_valuation
                    .map(|valuation| valuation.basis.as_str()),
            )
            .bind(
                v.price_match_valuation
                    .map(|valuation| valuation.fx_rate_id.into_uuid()),
            )
            .bind(v.user_search_filter_name.as_ref().map(AsRef::as_ref))
            .bind(v.feedback)
            .bind(v.shipping_match_valuation.map(|v| v.fx_rate_id.into_uuid()))
            .bind(v.shipping_match_valuation.map(|v| v.captured_at))
            .bind(
                v.shipping_match_valuation
                    .map(|v| v.maximum_per_item.currency.as_str()),
            )
            .bind(
                v.shipping_match_valuation
                    .map(|v| i64::try_from(u64::from(v.maximum_per_item.monetary_amount)))
                    .transpose()
                    .map_err(|source| SearchFilterMatchRepositoryError::InsertFailed {
                        source: box_error(source),
                    })?,
            )
            .fetch_one(self.tx.connection())
            .await
            .map_err(|source| SearchFilterMatchRepositoryError::InsertFailed {
                source: box_error(source),
            })?;
        row.try_into().map_err(
            |source| SearchFilterMatchRepositoryError::InvalidPersistedState {
                source: box_error(source),
            },
        )
    }
    async fn update(
        &mut self,
        v: &SearchFilterProductListingMatch,
    ) -> Result<PersistedSearchFilterMatch, SearchFilterMatchRepositoryError> {
        let id = v.user_search_filter_id.into_uuid();
        let mut query = QueryBuilder::<Postgres>::new(
            "UPDATE search_filter_matches SET user_search_filter_name=$3,feedback=$4,updated=aura_advance_updated(updated) WHERE user_search_filter_id=$1 AND product_listing_id=$2 RETURNING ",
        );
        query.push(MATCH_COLUMNS);
        let row = query
            .build_query_as::<MatchRow>()
            .bind(id)
            .bind(v.product_listing_id.into_uuid())
            .bind(v.user_search_filter_name.as_ref().map(AsRef::as_ref))
            .bind(v.feedback)
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|source| SearchFilterMatchRepositoryError::UpdateFailed {
                source: box_error(source),
            })?
            .ok_or_else(|| SearchFilterMatchRepositoryError::UpdateFailed {
                source: static_error("search filter match disappeared before update"),
            })?;
        row.try_into().map_err(
            |source| SearchFilterMatchRepositoryError::InvalidPersistedState {
                source: box_error(source),
            },
        )
    }
}
