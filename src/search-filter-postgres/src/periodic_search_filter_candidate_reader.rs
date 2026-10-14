use crate::mapping::{
    SearchFilterRowMappingError, matching_evidence_policy, name, product_search_from_json, state,
};
use application::error::box_error;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    PeriodicSearchFilterCandidate, PeriodicSearchFilterCandidatePageRequest,
    PeriodicSearchFilterCandidateReadError, PeriodicSearchFilterCandidateReader,
};
use sqlx::{FromRow, PgPool};
use time::OffsetDateTime;

#[derive(Clone)]
pub struct SqlxPeriodicSearchFilterCandidateReader {
    pool: PgPool,
}

impl SqlxPeriodicSearchFilterCandidateReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct PeriodicCandidateRow {
    member_filter_id: uuid::Uuid,
    member_user_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    user_id: uuid::Uuid,
    name: String,
    state: String,
    search: serde_json::Value,
    embedding: Option<Vec<f32>>,
    matching_evidence_policy: String,
    created: OffsetDateTime,
    matched_through: OffsetDateTime,
    version: i64,
}

#[async_trait::async_trait]
impl PeriodicSearchFilterCandidateReader for SqlxPeriodicSearchFilterCandidateReader {
    async fn find_active_page(
        &self,
        request: PeriodicSearchFilterCandidatePageRequest,
    ) -> Result<Vec<PeriodicSearchFilterCandidate>, PeriodicSearchFilterCandidateReadError> {
        let after = request.after.map(UserSearchFilterId::into_uuid);
        let limit = i64::try_from(request.page_size).map_err(|source| {
            PeriodicSearchFilterCandidateReadError::ReadFailed {
                source: box_error(source),
            }
        })?;
        sqlx::query_as::<_, PeriodicCandidateRow>(
            r#"
            WITH candidates AS (
                SELECT f.*, COALESCE(p.matched_through, f.created) AS matched_through,
                       f.user_search_filter_id AS member_filter_id, f.user_id AS member_user_id
                FROM search_filters f LEFT JOIN search_filter_periodic_match_state p USING(user_search_filter_id)
                WHERE NOT EXISTS (SELECT 1 FROM search_filter_matching_runs WHERE window_end = $1)
                    AND f.state = 'ACTIVE' AND f.created <= $1 AND COALESCE(p.matched_through, f.created) < $1
                UNION ALL
                SELECT f.*, (r.snapshot->>'matched_through')::timestamptz AS matched_through,
                       r.user_search_filter_id AS member_filter_id, r.user_id AS member_user_id
                FROM search_filter_matching_run_filters r
                CROSS JOIN LATERAL jsonb_populate_record(NULL::search_filters, r.snapshot) f
                WHERE r.window_end = $1 AND NOT r.terminal
                    AND EXISTS (SELECT 1 FROM search_filter_matching_runs run WHERE run.window_end = $1 AND run.status = 'RUNNING')
            )
            SELECT member_filter_id, member_user_id, user_search_filter_id, user_id, name, state, search, embedding, matching_evidence_policy, created, matched_through, version
            FROM candidates
            WHERE ($2::uuid IS NULL OR member_filter_id > $2)
            ORDER BY member_filter_id ASC
            LIMIT $3
        "#,
        )
        .bind(request.eligible_at_or_before)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(
            |source| PeriodicSearchFilterCandidateReadError::ReadFailed {
                source: box_error(source),
            },
        )?
        .into_iter()
        .map(|row| {
            if row.member_filter_id != row.user_search_filter_id
                || row.member_user_id != row.user_id
            {
                return Err(PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                    source: box_error(std::io::Error::other(
                        "periodic matching filter snapshot identity mismatch",
                    )),
                });
            }
            let search = product_search_from_json(row.search).map_err(|source| {
                PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?;
            let policy =
                matching_evidence_policy(&row.matching_evidence_policy).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?;
            // Validate the persisted capability snapshot without consulting entitlements.
            // Incompatible intent must not be silently removed from an executable filter.
            if search.enhanced_search_description.is_some() && !policy.permits_enhanced_intent() {
                return Err(
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(
                            SearchFilterRowMappingError::InvalidMatchingEvidencePolicy,
                        ),
                    },
                );
            }
            Ok(PeriodicSearchFilterCandidate {
                search_filter_id: UserSearchFilterId::try_from(row.user_search_filter_id).map_err(
                    |source| PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    },
                )?,
                user_id: user_core::user_id::UserId::try_from(row.user_id).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                name: name(row.name).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                version: row.version,
                state: state(&row.state).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                search,
                embedding: row.embedding,
                matching_evidence_policy: policy,
                created: row.created,
                matched_through: row.matched_through,
            })
        })
        .collect()
    }
}
