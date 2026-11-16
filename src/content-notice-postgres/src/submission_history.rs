use crate::sql_error::is_temporary;
use application::error::box_error;
use content_notice_core::content_notice_target::ContentNoticeTarget;
use content_notice_service::ports::{
    ContentNoticeSubmissionFingerprint, ContentNoticeSubmissionHistory,
    ContentNoticeSubmissionHistoryError, ContentNoticeSubmissionHistoryFactory, RecentSubmission,
    SubmissionGroup,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use time::OffsetDateTime;

/// Reads the submission history of a group from `content_notices`. An address key is always
/// computed by PostgreSQL with `lower()`, the same expression as the stored
/// `submitter_email_key`, so the lock, the read and the stored rows agree for every address.
/// Notices without an address have no key and are grouped by their target.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxContentNoticeSubmissionHistoryFactory;

struct History<'a> {
    connection: &'a mut PgConnection,
}

impl SqlxContentNoticeSubmissionHistoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ContentNoticeSubmissionHistoryFactory<SqlxTransaction>
    for SqlxContentNoticeSubmissionHistoryFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ContentNoticeSubmissionHistory + 'a {
        History {
            connection: tx.connection(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct RecentSubmissionRow {
    submission_fingerprint: Vec<u8>,
    created: OffsetDateTime,
}

#[async_trait::async_trait]
impl ContentNoticeSubmissionHistory for History<'_> {
    async fn lock_group(
        &mut self,
        group: SubmissionGroup<'_>,
    ) -> Result<(), ContentNoticeSubmissionHistoryError> {
        let query = match group {
            SubmissionGroup::Address(email) => sqlx::query(
                "SELECT pg_advisory_xact_lock(hashtextextended('content_notice_submitter:' || lower($1), 0))",
            )
            .bind(email.as_str()),
            SubmissionGroup::TargetWithoutAddress(target) => sqlx::query(
                "SELECT pg_advisory_xact_lock(hashtextextended('content_notice_anonymous_target:' || $1, 0))",
            )
            .bind(target_lock_key(target)),
        };
        query
            .execute(&mut *self.connection)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    async fn submissions_after(
        &mut self,
        group: SubmissionGroup<'_>,
        after: OffsetDateTime,
    ) -> Result<Vec<RecentSubmission>, ContentNoticeSubmissionHistoryError> {
        let query = match group {
            SubmissionGroup::Address(email) => sqlx::query_as::<_, RecentSubmissionRow>(
                "SELECT submission_fingerprint, created FROM content_notices WHERE submitter_email_key = lower($1) AND created > $2",
            )
            .bind(email.as_str())
            .bind(after),
            // The target type follows from whether a listing is named, so the identifiers
            // select exactly the notices about this target.
            SubmissionGroup::TargetWithoutAddress(target) => sqlx::query_as::<_, RecentSubmissionRow>(
                "SELECT submission_fingerprint, created FROM content_notices WHERE submitter_email_key IS NULL AND listing_source_id = $1 AND product_listing_id IS NOT DISTINCT FROM $2 AND created > $3",
            )
            .bind(target.listing_source_id().into_uuid())
            .bind(target.product_listing_id().map(|id| id.into_uuid()))
            .bind(after),
        };
        query
            .fetch_all(&mut *self.connection)
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(recent_submission)
            .collect()
    }
}

/// Names the target for the advisory lock of its anonymous notices.
fn target_lock_key(target: ContentNoticeTarget) -> String {
    match target.product_listing_id() {
        Some(product_listing_id) => format!("{}:{}", target.type_code(), product_listing_id),
        None => format!("{}:{}", target.type_code(), target.listing_source_id()),
    }
}

fn recent_submission(
    row: RecentSubmissionRow,
) -> Result<RecentSubmission, ContentNoticeSubmissionHistoryError> {
    let digest = <[u8; 32]>::try_from(row.submission_fingerprint).map_err(|_| {
        ContentNoticeSubmissionHistoryError::InvalidPersistedState {
            source: box_error(InvalidFingerprint),
        }
    })?;
    Ok(RecentSubmission {
        fingerprint: ContentNoticeSubmissionFingerprint::from_sha256(digest),
        created: row.created,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("persisted submission fingerprint is not a SHA-256 digest")]
struct InvalidFingerprint;

/// A failed statement that is not temporary, such as an undecodable column, is invalid state.
fn unavailable(error: sqlx::Error) -> ContentNoticeSubmissionHistoryError {
    if is_temporary(&error) {
        return ContentNoticeSubmissionHistoryError::TemporarilyUnavailable {
            source: box_error(error),
        };
    }
    ContentNoticeSubmissionHistoryError::InvalidPersistedState {
        source: box_error(error),
    }
}
