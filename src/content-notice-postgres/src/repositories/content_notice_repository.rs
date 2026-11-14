use crate::mapping::{CONTENT_NOTICE_COLUMNS, ContentNoticeRow, MappingError, content_notice};
use crate::sql_error::is_temporary;
use application::error::box_error;
use content_notice_core::{content_notice::ContentNotice, content_notice_id::ContentNoticeId};
use content_notice_service::ports::{
    ContentNoticeRepository, ContentNoticeRepositoryError, ContentNoticeRepositoryFactory,
    ContentNoticeSubmissionFingerprint, VersionedContentNotice,
};
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, PgConnection};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxContentNoticeRepositoryFactory;

struct Repository<'a> {
    connection: &'a mut PgConnection,
}

impl SqlxContentNoticeRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ContentNoticeRepositoryFactory<SqlxTransaction> for SqlxContentNoticeRepositoryFactory {
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ContentNoticeRepository + 'a {
        Repository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ContentNoticeRepository for Repository<'_> {
    async fn insert(
        &mut self,
        notice: &ContentNotice,
        fingerprint: ContentNoticeSubmissionFingerprint,
        created: OffsetDateTime,
    ) -> Result<VersionedContentNotice, ContentNoticeRepositoryError> {
        let target = notice.target();
        let snapshot = notice.target_snapshot();
        let product_listing = snapshot.product_listing.as_ref();
        let submitter = notice.submitter();
        sqlx::query_as::<_, ContentNoticeRow>(AssertSqlSafe(format!(
            "INSERT INTO content_notices(content_notice_id, target_type, listing_source_id, product_listing_id, listing_source_name, listing_source_url, listing_source_partner, product_listing_title, product_listing_url, submitter_role, reason_category, explanation, additional_urls, submitter_name, submitter_email, submitter_language, good_faith_statement_version, submission_fingerprint, business_state, created, updated) VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $20) RETURNING {CONTENT_NOTICE_COLUMNS}"
        )))
        .bind(notice.id().into_uuid())
        .bind(target.target_type().as_str())
        .bind(target.listing_source_id().into_uuid())
        .bind(target.product_listing_id().map(|id| id.into_uuid()))
        .bind(snapshot.listing_source.name.as_ref())
        .bind(snapshot.listing_source.url.as_ref().map(|url| url.as_str()))
        .bind(snapshot.listing_source.partner)
        .bind(
            product_listing
                .and_then(|listing| listing.title.as_ref())
                .map(|title| title.as_str()),
        )
        .bind(product_listing.map(|listing| listing.url.as_str()))
        .bind(notice.submitter_role().as_str())
        .bind(notice.reason_category().as_str())
        .bind(notice.explanation().map(|explanation| explanation.as_str()))
        .bind(
            notice
                .additional_urls()
                .as_slice()
                .iter()
                .map(|url| url.as_str())
                .collect::<Vec<_>>(),
        )
        .bind(submitter.name.as_str())
        .bind(submitter.email.as_str())
        .bind(submitter.language.as_str())
        .bind(notice.good_faith_statement_version().as_str())
        .bind(fingerprint.as_bytes().as_slice())
        .bind(notice.state().as_str())
        .bind(created)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(write)
        .and_then(|row| content_notice(row).map_err(invalid))
    }

    async fn find_by_id(
        &mut self,
        id: ContentNoticeId,
    ) -> Result<Option<VersionedContentNotice>, ContentNoticeRepositoryError> {
        sqlx::query_as::<_, ContentNoticeRow>(AssertSqlSafe(format!(
            "SELECT {CONTENT_NOTICE_COLUMNS} FROM content_notices WHERE content_notice_id = $1"
        )))
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(read)?
        .map(content_notice)
        .transpose()
        .map_err(invalid)
    }
}

fn read(error: sqlx::Error) -> ContentNoticeRepositoryError {
    ContentNoticeRepositoryError::TemporarilyUnavailable {
        source: box_error(error),
    }
}

fn write(error: sqlx::Error) -> ContentNoticeRepositoryError {
    if is_temporary(&error) {
        return read(error);
    }
    ContentNoticeRepositoryError::Internal {
        source: box_error(error),
    }
}

fn invalid(error: MappingError) -> ContentNoticeRepositoryError {
    ContentNoticeRepositoryError::InvalidPersistedState {
        source: box_error(error),
    }
}
