use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_core::product_id::ProductId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    SearchFilterMatchNotificationSource, SearchFilterMatchNotificationSourceReadError,
    SearchFilterMatchNotificationSourceReader, SearchFilterMatchNotificationSourceReaderFactory,
};
use sqlx::FromRow;
use time::OffsetDateTime;
use user_core::user_id::UserId;

use crate::mapping::{name, user_search_filter_uuid};

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMatchNotificationSourceReaderFactory;

struct SqlxSearchFilterMatchNotificationSourceReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMatchNotificationSourceReaderFactory<SqlxTransaction>
    for SqlxSearchFilterMatchNotificationSourceReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMatchNotificationSourceReader + 'tx {
        SqlxSearchFilterMatchNotificationSourceReader { tx }
    }
}

#[derive(Debug, FromRow)]
struct SearchFilterMatchNotificationSourceRow {
    user_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    product_id: uuid::Uuid,
    origin_event_id: uuid::Uuid,
    created: OffsetDateTime,
    user_search_filter_name: String,
    external_delivery_requested: bool,
}

#[async_trait::async_trait]
impl SearchFilterMatchNotificationSourceReader
    for SqlxSearchFilterMatchNotificationSourceReader<'_>
{
    async fn find_source(
        &mut self,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        product_id: ProductId,
        origin_event_id: EventId,
    ) -> Result<
        Option<SearchFilterMatchNotificationSource>,
        SearchFilterMatchNotificationSourceReadError,
    > {
        let search_filter_id = user_search_filter_uuid(search_filter_id).map_err(|source| {
            SearchFilterMatchNotificationSourceReadError::ReadFailed {
                source: box_error(source),
            }
        })?;
        let row = sqlx::query_as::<_, SearchFilterMatchNotificationSourceRow>(
            r#"
            SELECT
                matched.user_id,
                matched.user_search_filter_id,
                matched.product_id,
                matched.origin_event_id,
                matched.created,
                COALESCE(matched.user_search_filter_name, filter.name) AS user_search_filter_name,
                filter.notifications AS external_delivery_requested
            FROM search_filter_matches matched
            JOIN search_filters filter
                ON filter.user_search_filter_id = matched.user_search_filter_id
            WHERE matched.user_id = $1
                AND matched.user_search_filter_id = $2
                AND matched.product_id = $3
                AND matched.origin_event_id = $4
            "#,
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(search_filter_id)
        .bind(uuid::Uuid::from(product_id))
        .bind(uuid::Uuid::from(origin_event_id))
        .fetch_optional(self.tx.connection())
        .await
        .map_err(
            |source| SearchFilterMatchNotificationSourceReadError::ReadFailed {
                source: box_error(source),
            },
        )?;

        row.map(|row| {
            Ok(SearchFilterMatchNotificationSource {
                user_id: UserId::from(row.user_id),
                search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
                search_filter_name: name(row.user_search_filter_name).map_err(|source| {
                    SearchFilterMatchNotificationSourceReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                product_id: ProductId::from(row.product_id),
                origin_event_id: row.origin_event_id.into(),
                matched_at: row.created,
                external_delivery_requested: row.external_delivery_requested,
            })
        })
        .transpose()
    }
}
