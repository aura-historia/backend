use application::error::box_error;
use content_notice_core::submitter::SubmitterEmail;
use content_notice_service::ports::{
    ContentNoticeSubmissionFingerprint, ContentNoticeSubmissionHistory,
    ContentNoticeSubmissionHistoryError, ContentNoticeSubmissionHistoryFactory, RecentSubmission,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use time::OffsetDateTime;

/// Reads the submission history of an address from `content_notices`. The address key is
/// always computed by PostgreSQL with `lower()`, the same expression as the stored
/// `submitter_email_key`, so the lock, the read and the stored rows agree for every address.
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
    async fn lock_address(
        &mut self,
        email: &SubmitterEmail,
    ) -> Result<(), ContentNoticeSubmissionHistoryError> {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('content_notice_submitter:' || lower($1), 0))",
        )
        .bind(email.as_str())
        .execute(&mut *self.connection)
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    async fn submissions_after(
        &mut self,
        email: &SubmitterEmail,
        after: OffsetDateTime,
    ) -> Result<Vec<RecentSubmission>, ContentNoticeSubmissionHistoryError> {
        sqlx::query_as::<_, RecentSubmissionRow>(
            "SELECT submission_fingerprint, created FROM content_notices WHERE submitter_email_key = lower($1) AND created > $2",
        )
        .bind(email.as_str())
        .bind(after)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(unavailable)?
        .into_iter()
        .map(recent_submission)
        .collect()
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

fn unavailable(error: sqlx::Error) -> ContentNoticeSubmissionHistoryError {
    ContentNoticeSubmissionHistoryError::TemporarilyUnavailable {
        source: box_error(error),
    }
}
