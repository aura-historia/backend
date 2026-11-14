use crate::mapping::{
    CONTENT_NOTICE_SUMMARY_COLUMNS, ContentNoticeSummaryRow, content_notice_summary,
};
use application::{error::box_error, pagination::Cursor};
use content_notice_service::{
    ports::{
        ContentNoticeAdminSearchReadError, ContentNoticeAdminSearchReader,
        ContentNoticeAdminSearchReaderFactory,
    },
    use_cases::queries::list_admin_content_notices::{
        ContentNoticeCursor, ContentNoticeSearch, ContentNoticeSortKey,
        ListAdminContentNoticesRequest, ListAdminContentNoticesResult, SortContentNoticeField,
    },
};
use domain_primitives::sort::SortOrder;
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};
use time::{Duration, OffsetDateTime};

const MIN_PAGE_SIZE: u64 = 1;
const MAX_PAGE_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxContentNoticeAdminSearchReaderFactory;

struct Reader<'a> {
    connection: &'a mut PgConnection,
}

impl SqlxContentNoticeAdminSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ContentNoticeAdminSearchReaderFactory<SqlxTransaction>
    for SqlxContentNoticeAdminSearchReaderFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ContentNoticeAdminSearchReader + 'a {
        Reader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ContentNoticeAdminSearchReader for Reader<'_> {
    async fn search(
        &mut self,
        request: &ListAdminContentNoticesRequest,
    ) -> Result<ListAdminContentNoticesResult, ContentNoticeAdminSearchReadError> {
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
        let page_len = usize::try_from(size).map_err(internal)?;
        let sort = request.effective_sort();

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(CONTENT_NOTICE_SUMMARY_COLUMNS)
            .push(" FROM content_notices WHERE TRUE");
        push_filters(&mut builder, &request.search);
        if let Some(after) = cursor.search_after {
            push_after(&mut builder, sort.order, after);
        }
        push_order_by(&mut builder, sort.sort, sort.order);
        builder
            .push(" LIMIT ")
            .push_bind(i64::try_from(size + 1).map_err(internal)?);

        let mut rows = builder
            .build_query_as::<ContentNoticeSummaryRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| ContentNoticeAdminSearchReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;
        let has_more = rows.len() > page_len;
        rows.truncate(page_len);
        let items = rows
            .into_iter()
            .map(content_notice_summary)
            .collect::<Result<Vec<_>, _>>()
            .map_err(
                |source| ContentNoticeAdminSearchReadError::InvalidReadModel {
                    source: box_error(source),
                },
            )?;
        let search_after = items
            .last()
            .filter(|_| has_more)
            .map(|last| ContentNoticeCursor {
                scope: request.scope_binding(),
                sort_key: ContentNoticeSortKey::Created(last.created),
                id: last.id,
            });

        Ok(ListAdminContentNoticesResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, search: &ContentNoticeSearch) {
    if let Some(target_type) = search.target_type {
        builder
            .push(" AND target_type = ")
            .push_bind(target_type.as_str());
    }
    if let Some(listing_source_id) = search.listing_source_id {
        builder
            .push(" AND listing_source_id = ")
            .push_bind(listing_source_id.into_uuid());
    }
    if let Some(product_listing_id) = search.product_listing_id {
        builder
            .push(" AND product_listing_id = ")
            .push_bind(product_listing_id.into_uuid());
    }
    if let Some(reason_category) = search.reason_category {
        builder
            .push(" AND reason_category = ")
            .push_bind(reason_category.as_str());
    }
    if let Some(submitter_role) = search.submitter_role {
        builder
            .push(" AND submitter_role = ")
            .push_bind(submitter_role.as_str());
    }
    if let Some(created) = search.created {
        if let Some(min) = created.min {
            builder
                .push(" AND created >= ")
                .push_bind(ceil_to_microsecond(min));
        }
        if let Some(max) = created.max {
            builder.push(" AND created <= ").push_bind(max);
        }
    }
}

/// Continues strictly after the cursor's position. Sort column and tie-breaker share one
/// direction, so a row comparison expresses it and stays usable for the `(created,
/// content_notice_id)` indexes.
fn push_after(builder: &mut QueryBuilder<Postgres>, order: SortOrder, after: ContentNoticeCursor) {
    let ContentNoticeSortKey::Created(created) = after.sort_key;
    builder
        .push(" AND (created, content_notice_id) ")
        .push(match order {
            SortOrder::Asc => ">",
            SortOrder::Desc => "<",
        })
        .push(" (")
        .push_bind(created)
        .push(", ")
        .push_bind(after.id.into_uuid())
        .push(")");
}

fn push_order_by(
    builder: &mut QueryBuilder<Postgres>,
    field: SortContentNoticeField,
    order: SortOrder,
) {
    let column = match field {
        SortContentNoticeField::Created => "created",
    };
    let direction = match order {
        SortOrder::Asc => "ASC",
        SortOrder::Desc => "DESC",
    };
    builder.push(format!(
        " ORDER BY {column} {direction}, content_notice_id {direction}"
    ));
}

fn internal(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ContentNoticeAdminSearchReadError {
    ContentNoticeAdminSearchReadError::Internal {
        source: box_error(error),
    }
}

/// PostgreSQL stores microseconds and the bind truncates finer values, which would move an
/// inclusive lower bound earlier and admit a row before it.
fn ceil_to_microsecond(instant: OffsetDateTime) -> OffsetDateTime {
    match instant.nanosecond() % 1_000 {
        0 => instant,
        below => instant + Duration::nanoseconds(i64::from(1_000 - below)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_round_a_lower_bound_up_to_the_next_stored_microsecond() {
        assert_eq!(
            datetime!(2026-11-13 10:00:00.000001 UTC),
            ceil_to_microsecond(datetime!(2026-11-13 10:00:00.0000005 UTC))
        );
        assert_eq!(
            datetime!(2026-11-13 10:00:00.123456 UTC),
            ceil_to_microsecond(datetime!(2026-11-13 10:00:00.123456 UTC))
        );
    }
}
