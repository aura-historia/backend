use crate::mapping::{name, user_search_filter_uuid};
use common::{
    error::boxed::box_error, postgres::SqlxTransaction, user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
};
use search_filter_service::ports::{
    ActiveSearchFilterMatchCandidate, ActiveSearchFilterMatchCandidateReadError,
    ActiveSearchFilterMatchCandidateReader, ActiveSearchFilterMatchCandidateReaderFactory,
    SearchFilterMatchCandidate,
};
use sqlx::FromRow;

#[derive(Debug, Clone, Default)]
pub struct SqlxActiveSearchFilterMatchCandidateReaderFactory;

struct SqlxActiveSearchFilterMatchCandidateReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl ActiveSearchFilterMatchCandidateReaderFactory<SqlxTransaction>
    for SqlxActiveSearchFilterMatchCandidateReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ActiveSearchFilterMatchCandidateReader + 'tx {
        SqlxActiveSearchFilterMatchCandidateReader { tx }
    }
}

#[derive(Debug, FromRow)]
struct ActiveCandidateRow {
    user_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    name: String,
    enhanced_match_reason: Option<String>,
}

#[async_trait::async_trait]
impl ActiveSearchFilterMatchCandidateReader for SqlxActiveSearchFilterMatchCandidateReader<'_> {
    async fn find_active(
        &mut self,
        candidates: &[SearchFilterMatchCandidate],
    ) -> Result<Vec<ActiveSearchFilterMatchCandidate>, ActiveSearchFilterMatchCandidateReadError>
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
                |source| ActiveSearchFilterMatchCandidateReadError::ReadFailed {
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

        sqlx::query_as::<_, ActiveCandidateRow>(
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
            WHERE filter.state = 'ACTIVE'
            "#,
        )
        .bind(user_ids)
        .bind(filter_ids)
        .bind(reasons)
        .fetch_all(self.tx.connection())
        .await
        .map_err(
            |source| ActiveSearchFilterMatchCandidateReadError::ReadFailed {
                source: box_error(source),
            },
        )?
        .into_iter()
        .map(|row| {
            Ok(ActiveSearchFilterMatchCandidate {
                user_id: UserId::from(row.user_id),
                search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
                search_filter_name: name(row.name).map_err(|source| {
                    ActiveSearchFilterMatchCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
            })
        })
        .collect()
    }
}
