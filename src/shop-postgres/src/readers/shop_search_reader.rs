use crate::mapping::{
    ShopSummaryRow, countries_for_continents, shop_summary_columns, sort_value_for_summary_row,
};
use common::pagination::cursor::Cursor;
use common::postgres::SqlxTransaction;
use common::sort::{Sort, SortOrder};
use serde_json::Value;
use shop_core::shop_search::ShopSearch;
use shop_core::sort_shop_field::SortShopField;
use shop_service::ports::{ShopSearchReadError, ShopSearchReader, ShopSearchReaderFactory};
use shop_service::use_cases::queries::search_shops::{SearchShopsRequest, SearchShopsResult};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use time::OffsetDateTime;

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
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(|_| ShopSearchReadError::Internal)?;
        let limit = i64::try_from(size + 1).map_err(|_| ShopSearchReadError::Internal)?;
        let sort = normalize_sort(request.sort);

        let base = format!("SELECT {} FROM shops WHERE TRUE", shop_summary_columns());
        let mut builder = QueryBuilder::<Postgres>::new(base);
        push_filters(&mut builder, &request.search);
        push_cursor(&mut builder, sort, cursor.search_after.as_ref())?;
        push_order(&mut builder, sort);
        builder.push(" LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<ShopSummaryRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|_| ShopSearchReadError::TemporarilyUnavailable)?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let search_after = if has_more {
            rows.last().map(|row| {
                serde_json::json!([
                    sort_value_for_summary_row(sort.sort, row),
                    row.shop_id.to_string()
                ])
            })
        } else {
            None
        };

        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ShopSearchReadError::InvalidReadModel)?;

        Ok(SearchShopsResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn normalize_sort(sort: Option<Sort<SortShopField>>) -> Sort<SortShopField> {
    match sort {
        Some(Sort {
            sort: SortShopField::Score,
            ..
        })
        | None => Sort {
            sort: SortShopField::Name,
            order: SortOrder::Asc,
        },
        Some(sort) => sort,
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

fn push_cursor(
    builder: &mut QueryBuilder<'_, Postgres>,
    sort: Sort<SortShopField>,
    search_after: Option<&Value>,
) -> Result<(), ShopSearchReadError> {
    let Some(search_after) = search_after else {
        return Ok(());
    };
    let cursor = SearchCursor::parse(search_after, sort.sort)?;
    let primary_comparison = match sort.order {
        SortOrder::Asc => " > ",
        SortOrder::Desc => " < ",
    };

    builder.push(" AND (").push(field_sql(sort.sort));
    match cursor.primary {
        CursorPrimary::Text(value) => {
            builder
                .push(primary_comparison)
                .push_bind(value.clone())
                .push(" OR (")
                .push(field_sql(sort.sort))
                .push(" = ")
                .push_bind(value)
                .push(" AND shop_id > ")
                .push_bind(cursor.shop_id)
                .push("))");
        }
        CursorPrimary::Time(value) => {
            builder
                .push(primary_comparison)
                .push_bind(value)
                .push(" OR (")
                .push(field_sql(sort.sort))
                .push(" = ")
                .push_bind(value)
                .push(" AND shop_id > ")
                .push_bind(cursor.shop_id)
                .push("))");
        }
    }

    Ok(())
}

fn push_order(builder: &mut QueryBuilder<'_, Postgres>, sort: Sort<SortShopField>) {
    builder.push(" ORDER BY ").push(field_sql(sort.sort));
    match sort.order {
        SortOrder::Asc => builder.push(" ASC"),
        SortOrder::Desc => builder.push(" DESC"),
    };
    builder.push(", shop_id ASC");
}

fn field_sql(field: SortShopField) -> &'static str {
    match field {
        SortShopField::Score | SortShopField::Name => "name",
        SortShopField::Updated => "updated",
        SortShopField::Created => "created",
    }
}

struct SearchCursor {
    primary: CursorPrimary,
    shop_id: uuid::Uuid,
}

enum CursorPrimary {
    Text(String),
    Time(OffsetDateTime),
}

impl SearchCursor {
    fn parse(value: &Value, field: SortShopField) -> Result<Self, ShopSearchReadError> {
        let values = value
            .as_array()
            .ok_or(ShopSearchReadError::InvalidReadModel)?;
        if values.len() != 2 {
            return Err(ShopSearchReadError::InvalidReadModel);
        }
        let primary_value = values
            .first()
            .and_then(Value::as_str)
            .ok_or(ShopSearchReadError::InvalidReadModel)?;
        let shop_id = values
            .get(1)
            .and_then(Value::as_str)
            .ok_or(ShopSearchReadError::InvalidReadModel)
            .and_then(|value| {
                uuid::Uuid::parse_str(value).map_err(|_| ShopSearchReadError::InvalidReadModel)
            })?;
        let primary = match field {
            SortShopField::Score | SortShopField::Name => {
                CursorPrimary::Text(primary_value.to_owned())
            }
            SortShopField::Updated | SortShopField::Created => CursorPrimary::Time(
                OffsetDateTime::parse(
                    primary_value,
                    &time::format_description::well_known::Rfc3339,
                )
                .map_err(|_| ShopSearchReadError::InvalidReadModel)?,
            ),
        };

        Ok(Self { primary, shop_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::sort::SortOrder;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn should_default_score_sort_to_name_ascending() {
        let sort = normalize_sort(Some(Sort {
            sort: SortShopField::Score,
            order: SortOrder::Desc,
        }));

        assert_eq!(SortShopField::Name, sort.sort);
        assert_eq!(SortOrder::Asc, sort.order);
    }

    #[test]
    fn should_parse_name_cursor() {
        let shop_id = uuid::Uuid::new_v4();
        let cursor = json!(["Antik", shop_id.to_string()]);

        let result = SearchCursor::parse(&cursor, SortShopField::Name);

        assert!(
            matches!(result, Ok(SearchCursor { primary: CursorPrimary::Text(ref value), shop_id: parsed })
            if value == "Antik" && parsed == shop_id)
        );
    }

    #[test]
    fn should_parse_time_cursor() {
        let shop_id = uuid::Uuid::new_v4();
        let cursor = json!(["2026-01-01T00:00:00Z", shop_id.to_string()]);

        let result = SearchCursor::parse(&cursor, SortShopField::Updated);

        assert!(
            matches!(result, Ok(SearchCursor { primary: CursorPrimary::Time(value), shop_id: parsed })
            if value == datetime!(2026-01-01 0:00 UTC) && parsed == shop_id)
        );
    }

    #[test]
    fn should_reject_invalid_cursor_shape() {
        let result = SearchCursor::parse(&json!(["Antik"]), SortShopField::Name);

        assert!(matches!(result, Err(ShopSearchReadError::InvalidReadModel)));
    }
}
