use crate::mapping::{UserRow, bind_role, bind_tier, parse_role, parse_tier, user_columns};
use application::error::{box_error, static_error};
use application::pagination::{Cursor, SortKey};
use domain_primitives::sort::{Sort, SortOrder};
use platform_postgres::SqlxTransaction;
use platform_postgres::keyset::{self, KeysetColumn, KeysetValue, NullsOrder};
use sqlx::{Postgres, QueryBuilder};
use user_core::sort_user_field::SortUserField;

use user_service::ports::{UserSearchReadError, UserSearchReader, UserSearchReaderFactory};
use user_service::use_cases::queries::search_users::{
    SearchUsersRequest, SearchUsersResult, UserSearchCursor, UserSortKey, UserSummary,
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
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(|source| UserSearchReadError::Internal {
            source: box_error(source),
        })?;
        let limit = i64::try_from(size + 1).map_err(|source| UserSearchReadError::Internal {
            source: box_error(source),
        })?;
        let sort = request.effective_sort();
        let columns = keyset_columns(sort);

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder.push(user_columns()).push(" FROM users WHERE TRUE");
        push_filters(&mut builder, request);
        if let Some(after) = cursor.search_after {
            keyset::push_after(&mut builder, &columns, position(sort.sort, after)?).map_err(
                |source| UserSearchReadError::Internal {
                    source: box_error(source),
                },
            )?;
        }
        keyset::push_order_by(&mut builder, &columns);
        builder.push(" LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<UserRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|source| UserSearchReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let has_more = rows.len() > size_usize;
        rows.truncate(size_usize);
        let next_sort_key = rows
            .last()
            .filter(|_| has_more)
            .map(|row| sort_key(sort.sort, row))
            .transpose()?;
        let items = rows
            .into_iter()
            .map(UserSummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| UserSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after =
            next_sort_key
                .zip(items.last())
                .map(|(sort_key, last)| UserSearchCursor {
                    scope: request.scope_binding(),
                    sort_key,
                    id: last.user_id,
                });

        Ok(SearchUsersResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn keyset_columns(sort: Sort<SortUserField>) -> Vec<KeysetColumn> {
    let order = sort.order;
    // PostgreSQL's default NULL placement, which this search has always used.
    let nulls = match order {
        SortOrder::Asc => NullsOrder::Last,
        SortOrder::Desc => NullsOrder::First,
    };
    let first_name = KeysetColumn::nullable("first_name", order, nulls);
    let last_name = KeysetColumn::nullable("last_name", order, nulls);
    let mut columns = match sort.sort {
        SortUserField::Name => vec![first_name, last_name],
        SortUserField::Email => vec![KeysetColumn::required("email", order)],
        SortUserField::FirstName => vec![first_name],
        SortUserField::LastName => vec![last_name],
        SortUserField::Tier => vec![KeysetColumn::required("tier", order)],
        SortUserField::Role => vec![KeysetColumn::required("role", order)],
        SortUserField::Created => vec![KeysetColumn::required("created", order)],
        SortUserField::Updated => vec![KeysetColumn::required("updated", order)],
    };
    columns.push(KeysetColumn::required("user_id", SortOrder::Asc));
    columns
}

/// Stored values of the sorted fields, exactly as PostgreSQL orders them.
fn sort_key(field: SortUserField, row: &UserRow) -> Result<UserSortKey, UserSearchReadError> {
    let invalid = |source| UserSearchReadError::InvalidReadModel {
        source: box_error(source),
    };
    Ok(match field {
        SortUserField::Name => UserSortKey::Name {
            first_name: row.first_name.clone(),
            last_name: row.last_name.clone(),
        },
        SortUserField::Email => UserSortKey::Email(row.email.clone()),
        SortUserField::FirstName => UserSortKey::FirstName(row.first_name.clone()),
        SortUserField::LastName => UserSortKey::LastName(row.last_name.clone()),
        SortUserField::Tier => UserSortKey::Tier(parse_tier(&row.tier).map_err(invalid)?),
        SortUserField::Role => UserSortKey::Role(parse_role(&row.role).map_err(invalid)?),
        SortUserField::Created => UserSortKey::Created(row.created),
        SortUserField::Updated => UserSortKey::Updated(row.updated),
    })
}

fn position(
    field: SortUserField,
    after: UserSearchCursor,
) -> Result<Vec<KeysetValue>, UserSearchReadError> {
    if after.sort_key.field() != field {
        return Err(UserSearchReadError::Internal {
            source: static_error("user search cursor does not match the requested sort"),
        });
    }
    let mut position = match after.sort_key {
        UserSortKey::Name {
            first_name,
            last_name,
        } => vec![first_name.into(), last_name.into()],
        UserSortKey::Email(email) => vec![KeysetValue::Text(email)],
        UserSortKey::FirstName(name) | UserSortKey::LastName(name) => vec![name.into()],
        UserSortKey::Tier(tier) => vec![KeysetValue::Text(bind_tier(tier).to_owned())],
        UserSortKey::Role(role) => vec![KeysetValue::Text(bind_role(role).to_owned())],
        UserSortKey::Created(instant) | UserSortKey::Updated(instant) => {
            vec![KeysetValue::Timestamp(instant)]
        }
    };
    position.push(KeysetValue::Uuid(after.id.into_uuid()));
    Ok(position)
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &SearchUsersRequest) {
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
