use crate::mapping::{PartyRow, party_columns};
use application::error::{box_error, static_error};
use application::pagination::{Cursor, SortKey};
use domain_primitives::sort::{Sort, SortOrder};
use party_core::sort_party_field::SortPartyField;
use party_service::ports::{PartySearchReadError, PartySearchReader, PartySearchReaderFactory};
use party_service::use_cases::queries::search_parties::{
    PartySearchCursor, PartySortKey, PartySummary, SearchPartiesRequest, SearchPartiesResult,
};
use platform_postgres::SqlxTransaction;
use platform_postgres::keyset::{self, KeysetColumn, KeysetValue, NullsOrder};
use sqlx::{Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartySearchReaderFactory;

struct SqlxPartySearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartySearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartySearchReaderFactory<SqlxTransaction> for SqlxPartySearchReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartySearchReader + 'tx {
        SqlxPartySearchReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartySearchReader for SqlxPartySearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchPartiesRequest,
    ) -> Result<SearchPartiesResult, PartySearchReadError> {
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| PartySearchReadError::Internal {
                source: box_error(source),
            })?;
        let limit = i64::try_from(size + 1).map_err(|source| PartySearchReadError::Internal {
            source: box_error(source),
        })?;
        let sort = request.effective_sort();
        let columns = keyset_columns(sort);

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(party_columns())
            .push(" FROM parties WHERE TRUE");
        push_filters(&mut builder, request);
        if let Some(after) = cursor.search_after {
            keyset::push_after(&mut builder, &columns, position(sort.sort, after)?).map_err(
                |source| PartySearchReadError::Internal {
                    source: box_error(source),
                },
            )?;
        }
        keyset::push_order_by(&mut builder, &columns);
        builder.push(" LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<PartyRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|source| PartySearchReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let has_more = rows.len() > size_usize;
        rows.truncate(size_usize);
        let next_sort_key = rows
            .last()
            .filter(|_| has_more)
            .map(|row| row.sort_key(sort.sort));
        let items = rows
            .into_iter()
            .map(PartySummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartySearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after =
            next_sort_key
                .zip(items.last())
                .map(|(sort_key, last)| PartySearchCursor {
                    scope: request.scope_binding(),
                    sort_key,
                    id: last.party_id,
                });

        Ok(SearchPartiesResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn keyset_columns(sort: Sort<SortPartyField>) -> [KeysetColumn; 2] {
    let order = sort.order;
    let sorted = match sort.sort {
        SortPartyField::Name => KeysetColumn::required("name", order),
        SortPartyField::Email => KeysetColumn::nullable("email", order, NullsOrder::Last),
        SortPartyField::Phone => KeysetColumn::nullable("phone", order, NullsOrder::Last),
        SortPartyField::Created => KeysetColumn::required("created", order),
        SortPartyField::Updated => KeysetColumn::required("updated", order),
    };
    [sorted, KeysetColumn::required("party_id", SortOrder::Asc)]
}

fn position(
    field: SortPartyField,
    after: PartySearchCursor,
) -> Result<Vec<KeysetValue>, PartySearchReadError> {
    if after.sort_key.field() != field {
        return Err(PartySearchReadError::Internal {
            source: static_error("party search cursor does not match the requested sort"),
        });
    }
    let sort_value = match after.sort_key {
        PartySortKey::Name(name) => KeysetValue::Text(name),
        PartySortKey::Email(contact) | PartySortKey::Phone(contact) => KeysetValue::from(contact),
        PartySortKey::Created(instant) | PartySortKey::Updated(instant) => {
            KeysetValue::Timestamp(instant)
        }
    };
    Ok(vec![sort_value, KeysetValue::Uuid(after.id.into_uuid())])
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &SearchPartiesRequest) {
    let search = &request.search;

    if let Some(query) = &search.query {
        builder.push(" AND (");
        push_ilike(builder, "name", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "phone", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "email", query.as_ref());
        builder.push(")");
    }
    if let Some(query) = &search.name_query {
        builder.push(" AND ");
        push_ilike(builder, "name", query.as_ref());
    }
    if let Some(query) = &search.phone_query {
        builder.push(" AND ");
        push_ilike(builder, "phone", query.as_ref());
    }
    if let Some(query) = &search.email_query {
        builder.push(" AND ");
        push_ilike(builder, "email", query.as_ref());
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

fn push_ilike(builder: &mut QueryBuilder<Postgres>, column: &str, value: &str) {
    builder
        .push(column)
        .push(" ILIKE ")
        .push_bind(like_pattern(value))
        .push(r" ESCAPE E'\\'");
}

fn like_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}
