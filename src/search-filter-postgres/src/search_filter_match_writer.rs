use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_core::SearchFilterProductListingMatch;
use search_filter_service::ports::{
    SearchFilterMatchBatchPersistResult, SearchFilterMatchPersistOutcome,
    SearchFilterMatchWriteError, SearchFilterMatchWriter, SearchFilterMatchWriterFactory,
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
        product_match: &SearchFilterProductListingMatch,
    ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError> {
        let filter_id = product_match.user_search_filter_id.into_uuid();
        let inserted = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO search_filter_matches (
                user_id,
                user_search_filter_id,
                product_listing_id,
                origin_event_id,
                price_valuation_basis,
                price_fx_rate_id,
                user_search_filter_name,
                feedback,
                shipping_fx_rate_id,
                shipping_fx_captured_at,
                shipping_target_currency,
                shipping_maximum_minor
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (user_search_filter_id, product_listing_id) DO NOTHING
            RETURNING 1::bigint
            "#,
        )
        .bind(product_match.user_id.into_uuid())
        .bind(filter_id)
        .bind(product_match.product_listing_id.into_uuid())
        .bind(product_match.origin_event_id.into_uuid())
        .bind(
            product_match
                .price_match_valuation
                .map(|valuation| valuation.basis.as_str()),
        )
        .bind(
            product_match
                .price_match_valuation
                .map(|valuation| valuation.fx_rate_id.into_uuid()),
        )
        .bind(
            product_match
                .user_search_filter_name
                .as_ref()
                .map(AsRef::as_ref),
        )
        .bind(product_match.feedback)
        .bind(
            product_match
                .shipping_match_valuation
                .map(|v| v.fx_rate_id.into_uuid()),
        )
        .bind(
            product_match
                .shipping_match_valuation
                .map(|v| v.captured_at),
        )
        .bind(
            product_match
                .shipping_match_valuation
                .map(|v| v.maximum_per_item.currency.as_str()),
        )
        .bind(shipping_maximum(product_match)?)
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

    async fn insert_for_run(
        &mut self,
        matches: &[SearchFilterProductListingMatch],
        end: time::OffsetDateTime,
    ) -> Result<SearchFilterMatchBatchPersistResult, SearchFilterMatchWriteError> {
        self.insert_batch(matches, Some(end)).await
    }
    async fn insert_all_if_absent(
        &mut self,
        matches: &[SearchFilterProductListingMatch],
    ) -> Result<SearchFilterMatchBatchPersistResult, SearchFilterMatchWriteError> {
        self.insert_batch(matches, None).await
    }
}
impl SqlxSearchFilterMatchWriter<'_> {
    async fn insert_batch(
        &mut self,
        product_matches: &[SearchFilterProductListingMatch],
        run_end: Option<time::OffsetDateTime>,
    ) -> Result<SearchFilterMatchBatchPersistResult, SearchFilterMatchWriteError> {
        if product_matches.is_empty() {
            return Ok(SearchFilterMatchBatchPersistResult::default());
        }

        let user_ids = product_matches
            .iter()
            .map(|value| value.user_id.into_uuid())
            .collect::<Vec<_>>();
        let filter_ids = product_matches
            .iter()
            .map(|value| value.user_search_filter_id.into_uuid())
            .collect::<Vec<_>>();
        let product_listing_ids = product_matches
            .iter()
            .map(|value| value.product_listing_id.into_uuid())
            .collect::<Vec<_>>();
        let event_ids = product_matches
            .iter()
            .map(|value| value.origin_event_id.into_uuid())
            .collect::<Vec<_>>();
        let bases = product_matches
            .iter()
            .map(|value| {
                value
                    .price_match_valuation
                    .map(|valuation| valuation.basis.as_str())
            })
            .collect::<Vec<_>>();
        let fx_rate_ids = product_matches
            .iter()
            .map(|value| {
                value
                    .price_match_valuation
                    .map(|valuation| valuation.fx_rate_id.into_uuid())
            })
            .collect::<Vec<_>>();
        let names = product_matches
            .iter()
            .map(|value| {
                value
                    .user_search_filter_name
                    .as_ref()
                    .map(ToString::to_string)
            })
            .collect::<Vec<_>>();
        let feedback = product_matches
            .iter()
            .map(|value| value.feedback)
            .collect::<Vec<_>>();
        let shipping_fx_rate_ids = product_matches
            .iter()
            .map(|value| {
                value
                    .shipping_match_valuation
                    .map(|v| v.fx_rate_id.into_uuid())
            })
            .collect::<Vec<_>>();
        let shipping_captured_at = product_matches
            .iter()
            .map(|value| value.shipping_match_valuation.map(|v| v.captured_at))
            .collect::<Vec<_>>();
        let shipping_currencies = product_matches
            .iter()
            .map(|value| {
                value
                    .shipping_match_valuation
                    .map(|v| v.maximum_per_item.currency.as_str())
            })
            .collect::<Vec<_>>();
        let shipping_maximums = product_matches
            .iter()
            .map(shipping_maximum)
            .collect::<Result<Vec<_>, _>>()?;

        let inserted = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO search_filter_matches (
                user_id, user_search_filter_id, product_listing_id, origin_event_id,
                price_valuation_basis, price_fx_rate_id, user_search_filter_name, feedback,
                shipping_fx_rate_id, shipping_fx_captured_at, shipping_target_currency,
                shipping_maximum_minor, matching_run_end
            )
            SELECT ids.*, $13::timestamptz FROM unnest(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::text[],
                $6::uuid[], $7::text[], $8::bool[], $9::uuid[], $10::timestamptz[], $11::text[], $12::bigint[]
            ) AS ids(user_id, user_search_filter_id, product_listing_id, origin_event_id,
                price_valuation_basis, price_fx_rate_id, user_search_filter_name, feedback,
                shipping_fx_rate_id, shipping_fx_captured_at, shipping_target_currency, shipping_maximum_minor)
            ON CONFLICT (user_search_filter_id, product_listing_id) DO NOTHING
            RETURNING 1::bigint
        "#,
        )
        .bind(user_ids)
        .bind(filter_ids)
        .bind(product_listing_ids)
        .bind(event_ids)
        .bind(bases)
        .bind(fx_rate_ids)
        .bind(names)
        .bind(feedback)
        .bind(shipping_fx_rate_ids)
        .bind(shipping_captured_at)
        .bind(shipping_currencies)
        .bind(shipping_maximums)
        .bind(run_end)
        .fetch_all(self.tx.connection())
        .await
        .map_err(|source| SearchFilterMatchWriteError::WriteFailed {
            source: box_error(source),
        })?
        .len();

        Ok(SearchFilterMatchBatchPersistResult {
            inserted,
            already_exists: product_matches.len() - inserted,
        })
    }
}

fn shipping_maximum(
    value: &SearchFilterProductListingMatch,
) -> Result<Option<i64>, SearchFilterMatchWriteError> {
    value
        .shipping_match_valuation
        .map(|v| i64::try_from(u64::from(v.maximum_per_item.monetary_amount)))
        .transpose()
        .map_err(|source| SearchFilterMatchWriteError::WriteFailed {
            source: box_error(source),
        })
}
