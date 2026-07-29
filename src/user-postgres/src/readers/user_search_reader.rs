use crate::mapping::{UserRow, bind_role, bind_tier, countries_for_continents, user_columns};
use common::error::boxed::box_error;
use common::pagination::cursor::Cursor;
use common::postgres::SqlxTransaction;
use common::sort::{Sort, SortOrder};
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder};
use user_core::sort_user_field::SortUserField;
use user_service::ports::{UserSearchReadError, UserSearchReader, UserSearchReaderFactory};
use user_service::use_cases::queries::search_users::{
    SearchUsersRequest, SearchUsersResult, UserSummary,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserSearchReaderFactory;

struct SqlxUserSearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxUserSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserSearchReaderFactory<SqlxTransaction> for SqlxUserSearchReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl UserSearchReader + 'tx {
        SqlxUserSearchReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserSearchReader for SqlxUserSearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchUsersRequest,
    ) -> Result<SearchUsersResult, UserSearchReadError> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            "SELECT {} FROM users WHERE TRUE",
            user_columns()
        ));
        push_filters(&mut builder, request);
        push_sort(&mut builder, request.sort);
        let cursor = request.cursor.clone().unwrap_or_default();
        builder
            .push(" LIMIT ")
            .push_bind(i64::try_from(cursor.size.min(100)).unwrap_or(100));

        let rows = builder
            .build_query_as::<UserRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|source| UserSearchReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let items = rows
            .into_iter()
            .map(UserSummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| UserSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;

        Ok(SearchUsersResult {
            items,
            cursor: Cursor::<Value> {
                search_after: None,
                size: cursor.size,
            },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, request: &SearchUsersRequest) {
    let search = &request.search;

    if let Some(query) = &search.query {
        builder
            .push(" AND (email ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()))
            .push(" OR first_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()))
            .push(" OR last_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()))
            .push(")");
    }
    if let Some(query) = &search.email_query {
        builder
            .push(" AND email ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()));
    }
    if let Some(query) = &search.first_name_query {
        builder
            .push(" AND first_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()));
    }
    if let Some(query) = &search.last_name_query {
        builder
            .push(" AND last_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()));
    }
    if !search.tier_query.is_empty() {
        let tiers = search
            .tier_query
            .iter()
            .copied()
            .map(bind_tier)
            .collect::<Vec<_>>();
        builder.push(" AND tier = ANY(").push_bind(tiers).push(")");
    }
    if !search.role_query.is_empty() {
        let roles = search
            .role_query
            .iter()
            .copied()
            .map(bind_role)
            .collect::<Vec<_>>();
        builder.push(" AND role = ANY(").push_bind(roles).push(")");
    }
    if !search.country_query.is_empty() {
        let countries = search
            .country_query
            .iter()
            .map(|country| country.alpha3().to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND structured_address_country = ANY(")
            .push_bind(countries)
            .push(")");
    }
    if !search.continent_query.is_empty() {
        let countries = countries_for_continents(search.continent_query.as_ref());
        builder
            .push(" AND structured_address_country = ANY(")
            .push_bind(countries)
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

fn push_sort(builder: &mut QueryBuilder<'_, Postgres>, sort: Option<Sort<SortUserField>>) {
    let sort = sort.unwrap_or(Sort {
        sort: SortUserField::Created,
        order: SortOrder::Desc,
    });
    let column = match sort.sort {
        SortUserField::Score => "created",
        SortUserField::Email => "email",
        SortUserField::FirstName => "first_name",
        SortUserField::LastName => "last_name",
        SortUserField::Tier => "tier",
        SortUserField::Role => "role",
        SortUserField::Created => "created",
        SortUserField::Updated => "updated",
    };
    let order = match sort.order {
        SortOrder::Asc => "ASC",
        SortOrder::Desc => "DESC",
    };

    builder
        .push(" ORDER BY ")
        .push(column)
        .push(" ")
        .push(order);
}
