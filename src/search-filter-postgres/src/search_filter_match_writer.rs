use crate::mapping::user_search_filter_uuid;
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_core::SearchFilterProductMatch;
use search_filter_service::ports::{
    SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError, SearchFilterMatchWriter,
    SearchFilterMatchWriterFactory,
};

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMatchWriterFactory;

struct SqlxSearchFilterMatchWriter<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMatchWriterFactory<SqlxTransaction> for SqlxSearchFilterMatchWriterFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMatchWriter + 'tx {
        SqlxSearchFilterMatchWriter { tx }
    }
}

#[async_trait::async_trait]
impl SearchFilterMatchWriter for SqlxSearchFilterMatchWriter<'_> {
    async fn insert_if_absent(
        &mut self,
        product_match: &SearchFilterProductMatch,
    ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError> {
        let filter_id =
            user_search_filter_uuid(product_match.user_search_filter_id).map_err(|source| {
                SearchFilterMatchWriteError::WriteFailed {
                    source: box_error(source),
                }
            })?;
        let inserted = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO search_filter_matches (
                user_id,
                user_search_filter_id,
                product_id,
                origin_event_id,
                price_valuation_basis,
                price_fx_rate_id,
                user_search_filter_name,
                enhanced_match_reason,
                feedback
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (user_search_filter_id, product_id) DO NOTHING
            RETURNING 1::bigint
            "#,
        )
        .bind(uuid::Uuid::from(product_match.user_id))
        .bind(filter_id)
        .bind(uuid::Uuid::from(product_match.product_id))
        .bind(uuid::Uuid::from(product_match.origin_event_id))
        .bind(
            product_match
                .price_match_valuation
                .map(|valuation| valuation.basis.as_db_str()),
        )
        .bind(
            product_match
                .price_match_valuation
                .map(|valuation| uuid::Uuid::from(valuation.fx_rate_id)),
        )
        .bind(
            product_match
                .user_search_filter_name
                .as_ref()
                .map(AsRef::as_ref),
        )
        .bind(
            product_match
                .enhanced_match_reason
                .as_ref()
                .map(AsRef::as_ref),
        )
        .bind(product_match.feedback)
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|source| SearchFilterMatchWriteError::WriteFailed {
            source: box_error(source),
        })?;

        Ok(match inserted {
            Some(_) => SearchFilterMatchPersistOutcome::Inserted,
            None => SearchFilterMatchPersistOutcome::AlreadyExists,
        })
    }
}
