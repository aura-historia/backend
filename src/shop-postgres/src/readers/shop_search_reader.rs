use crate::mapping::{ShopSummaryRow, countries_for_continents, shop_summary_columns};
use common::error::boxed::box_error;
use common::pagination::cursor::Cursor;
use common::sort::{Sort, SortOrder};
use platform_postgres::SqlxTransaction;
use shop_core::shop_id::ShopId;
use shop_core::sort_shop_field::SortShopField;
use shop_service::ports::{ShopSearchReadError, ShopSearchReader, ShopSearchReaderFactory};
use shop_service::shop_search::ShopSearch;
use shop_service::use_cases::queries::search_shops::{SearchShopsRequest, SearchShopsResult};
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxShopSearchReaderFactory;

struct SqlxShopSearchReader<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxShopSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ShopSearchReaderFactory<SqlxTransaction> for SqlxShopSearchReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl ShopSearchReader + 'tx {
        SqlxShopSearchReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ShopSearchReader for SqlxShopSearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchShopsRequest,
    ) -> Result<SearchShopsResult, ShopSearchReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(|source| ShopSearchReadError::Internal {
            source: box_error(source),
        })?;
        let limit = i64::try_from(size + 1).map_err(|source| ShopSearchReadError::Internal {
            source: box_error(source),
        })?;
        let sort = request.sort.unwrap_or(Sort {
            sort: SortShopField::Name,
            order: SortOrder::Asc,
        });

        let mut builder = QueryBuilder::<Postgres>::new("WITH filtered AS (SELECT ");
        builder
            .push(shop_summary_columns())
            .push(" FROM shops WHERE lifecycle = 'PUBLISHED'");
        push_filters(&mut builder, &request.search);
        builder.push("), ranked AS (SELECT filtered.*, row_number() OVER (ORDER BY ");
        builder.push(field_sql(sort.sort));
        push_order_direction(&mut builder, sort.order);
        builder.push(", shop_id ASC) AS rn FROM filtered) SELECT ");
        builder
            .push(shop_summary_columns())
            .push(" FROM ranked WHERE TRUE");
        if let Some(search_after) = cursor.search_after {
            builder.push(" AND rn > (SELECT rn FROM ranked WHERE shop_id = ");
            builder.push_bind(uuid::Uuid::from(search_after));
            builder.push(")");
        }
        builder.push(" ORDER BY rn LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<ShopSummaryRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(SqlxShopSearchError)?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let search_after = if has_more {
            rows.last().map(|row| ShopId::from(row.shop_id))
        } else {
            None
        };

        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| ShopSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;

        Ok(SearchShopsResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

struct SqlxShopSearchError(sqlx::Error);

impl From<SqlxShopSearchError> for ShopSearchReadError {
    fn from(error: SqlxShopSearchError) -> Self {
        let SqlxShopSearchError(source) = error;
        Self::TemporarilyUnavailable {
            source: box_error(source),
        }
    }
}

fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, search: &ShopSearch) {
    if let Some(text) = &search.shop_name_query {
        builder
            .push(" AND name ILIKE ")
            .push_bind(format!("%{}%", text.as_ref()));
    }

    if !search.shop_type_query.is_empty() {
        let values = search
            .shop_type_query
            .iter()
            .map(|value| crate::mapping::bind_shop_type(*value).to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND shop_type = ANY(")
            .push_bind(values)
            .push(")");
    }

    if !search.partner_status_query.is_empty() {
        let values = search
            .partner_status_query
            .iter()
            .map(|value| crate::mapping::bind_partner_status(*value).to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND partner_status = ANY(")
            .push_bind(values)
            .push(")");
    }

    if !search.countries.is_empty() {
        let values = search
            .countries
            .iter()
            .map(|country| country.alpha3().to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND structured_address_country = ANY(")
            .push_bind(values)
            .push(")");
    }

    if !search.continents.is_empty() {
        let values = countries_for_continents(search.continents.as_ref());
        builder
            .push(" AND structured_address_country = ANY(")
            .push_bind(values)
            .push(")");
    }

    if let Some(created) = search.created {
        if let Some(min) = created.min {
            builder.push(" AND created >= ").push_bind(min);
        }
        if let Some(max) = created.max {
            builder.push(" AND created <= ").push_bind(max);
        }
    }

    if let Some(updated) = search.updated {
        if let Some(min) = updated.min {
            builder.push(" AND updated >= ").push_bind(min);
        }
        if let Some(max) = updated.max {
            builder.push(" AND updated <= ").push_bind(max);
        }
    }
}

fn push_order_direction(builder: &mut QueryBuilder<'_, Postgres>, order: SortOrder) {
    match order {
        SortOrder::Asc => builder.push(" ASC"),
        SortOrder::Desc => builder.push(" DESC"),
    };
}

fn field_sql(field: SortShopField) -> &'static str {
    match field {
        SortShopField::Name => "name",
        SortShopField::Updated => "updated",
        SortShopField::Created => "created",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_shop_id_cursor_type() {
        let cursor = Cursor {
            size: 10,
            search_after: Some(ShopId::new()),
        };

        assert!(cursor.search_after.is_some());
    }
}
