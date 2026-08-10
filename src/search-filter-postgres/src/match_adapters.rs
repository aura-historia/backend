use crate::mapping::{name, user_search_filter_uuid};
use common::{
    error::boxed::box_error, postgres::SqlxTransaction, product_id::ProductId, user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
};
use search_filter_core::SearchFilterProductMatch;
use search_filter_service::ports::{
    SearchFilterMatchCandidate, SearchFilterMatchCandidateValidationError,
    SearchFilterMatchCandidateValidator, SearchFilterMatchCandidateValidatorFactory,
    SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError, SearchFilterMatchWriter,
    SearchFilterMatchWriterFactory, SearchFilterMonthlyMatchQuotaReadError,
    SearchFilterMonthlyMatchQuotaReader, SearchFilterMonthlyMatchQuotaReaderFactory,
    ValidatedSearchFilterMatchCandidate,
};
use sqlx::FromRow;
use time::OffsetDateTime;

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMatchCandidateValidatorFactory;

struct SqlxSearchFilterMatchCandidateValidator<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMatchCandidateValidatorFactory<SqlxTransaction>
    for SqlxSearchFilterMatchCandidateValidatorFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMatchCandidateValidator + 'tx {
        SqlxSearchFilterMatchCandidateValidator { tx }
    }
}

#[derive(Debug, FromRow)]
struct ValidatedCandidateRow {
    user_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    name: String,
    enhanced_match_reason: Option<String>,
}

#[async_trait::async_trait]
impl SearchFilterMatchCandidateValidator for SqlxSearchFilterMatchCandidateValidator<'_> {
    async fn validate_for_product(
        &mut self,
        _product_id: ProductId,
        candidates: &[SearchFilterMatchCandidate],
    ) -> Result<Vec<ValidatedSearchFilterMatchCandidate>, SearchFilterMatchCandidateValidationError>
    {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let user_ids = candidates
            .iter()
            .map(|candidate| uuid::Uuid::from(candidate.user_id))
            .collect::<Vec<_>>();
        let filter_ids = candidates
            .iter()
            .map(|candidate| user_search_filter_uuid(candidate.search_filter_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(
                |source| SearchFilterMatchCandidateValidationError::ValidationFailed {
                    source: box_error(source),
                },
            )?;
        let reasons = candidates
            .iter()
            .map(|candidate| {
                candidate
                    .enhanced_match_reason
                    .as_ref()
                    .map(ToString::to_string)
            })
            .collect::<Vec<_>>();

        sqlx::query_as::<_, ValidatedCandidateRow>(
            r#"
            SELECT
                filter.user_id,
                filter.user_search_filter_id,
                filter.name,
                candidate.enhanced_match_reason
            FROM unnest($1::uuid[], $2::uuid[], $3::text[])
                AS candidate(user_id, user_search_filter_id, enhanced_match_reason)
            JOIN search_filters filter
                ON filter.user_id = candidate.user_id
                AND filter.user_search_filter_id = candidate.user_search_filter_id
            WHERE filter.state = 'Active'
            "#,
        )
        .bind(user_ids)
        .bind(filter_ids)
        .bind(reasons)
        .fetch_all(self.tx.connection())
        .await
        .map_err(
            |source| SearchFilterMatchCandidateValidationError::ValidationFailed {
                source: box_error(source),
            },
        )?
        .into_iter()
        .map(|row| {
            Ok(ValidatedSearchFilterMatchCandidate {
                user_id: UserId::from(row.user_id),
                search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
                search_filter_name: name(row.name).map_err(|source| {
                    SearchFilterMatchCandidateValidationError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
            })
        })
        .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMonthlyMatchQuotaReaderFactory;

struct SqlxSearchFilterMonthlyMatchQuotaReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMonthlyMatchQuotaReaderFactory<SqlxTransaction>
    for SqlxSearchFilterMonthlyMatchQuotaReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
        SqlxSearchFilterMonthlyMatchQuotaReader { tx }
    }
}

#[async_trait::async_trait]
impl SearchFilterMonthlyMatchQuotaReader for SqlxSearchFilterMonthlyMatchQuotaReader<'_> {
    async fn count_for_user_in_month(
        &mut self,
        user_id: UserId,
        occurred_at: OffsetDateTime,
    ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM search_filter_matches
            WHERE user_id = $1
                AND created >= date_trunc('month', $2::timestamptz)
                AND created < date_trunc('month', $2::timestamptz) + INTERVAL '1 month'
            "#,
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(occurred_at)
        .fetch_one(self.tx.connection())
        .await
        .map_err(
            |source| SearchFilterMonthlyMatchQuotaReadError::ReadFailed {
                source: box_error(source),
            },
        )?;

        usize::try_from(count).map_err(|source| {
            SearchFilterMonthlyMatchQuotaReadError::ReadFailed {
                source: box_error(source),
            }
        })
    }
}

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
                user_search_filter_name,
                enhanced_match_reason,
                feedback
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
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
