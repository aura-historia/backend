//! Keyset pagination for dynamically sorted reads.
//!
//! One ordering renders both the `ORDER BY` clause and the predicate selecting rows strictly
//! after a position, so the two cannot disagree. A position carries the sort values of the last
//! returned row instead of referring to that row, so it stays valid when the row is deleted or
//! changed. The final column must be a unique, non-null tie-breaker.

use domain_primitives::sort::SortOrder;
use sqlx::types::{Uuid, time::OffsetDateTime};
use sqlx::{Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullsOrder {
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeysetColumn {
    expression: &'static str,
    order: SortOrder,
    nulls: Option<NullsOrder>,
}

impl KeysetColumn {
    /// A column declared `NOT NULL`.
    pub const fn required(expression: &'static str, order: SortOrder) -> Self {
        Self {
            expression,
            order,
            nulls: None,
        }
    }

    pub const fn nullable(expression: &'static str, order: SortOrder, nulls: NullsOrder) -> Self {
        Self {
            expression,
            order,
            nulls: Some(nulls),
        }
    }
}

/// Sort value of one ordering column at a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysetValue {
    Null,
    Text(String),
    Timestamp(OffsetDateTime),
    Uuid(Uuid),
}

impl From<Option<String>> for KeysetValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(Self::Null, Self::Text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("keyset position has {values} values for {columns} ordering columns")]
pub struct KeysetArityError {
    columns: usize,
    values: usize,
}

pub fn push_order_by(builder: &mut QueryBuilder<Postgres>, columns: &[KeysetColumn]) {
    builder.push(" ORDER BY ");
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(column.expression).push(match column.order {
            SortOrder::Asc => " ASC",
            SortOrder::Desc => " DESC",
        });
        if let Some(nulls) = column.nulls {
            builder.push(match nulls {
                NullsOrder::First => " NULLS FIRST",
                NullsOrder::Last => " NULLS LAST",
            });
        }
    }
}

/// Appends ` AND <row is strictly after position>` for the ordering `columns`.
pub fn push_after(
    builder: &mut QueryBuilder<Postgres>,
    columns: &[KeysetColumn],
    position: Vec<KeysetValue>,
) -> Result<(), KeysetArityError> {
    if columns.is_empty() || columns.len() != position.len() {
        return Err(KeysetArityError {
            columns: columns.len(),
            values: position.len(),
        });
    }

    builder.push(" AND ");
    push_leading_bound(builder, &columns[0], &position[0]);
    // (c1 beyond v1 OR (c1 = v1 AND (c2 beyond v2 OR (c2 = v2 AND ... (cn beyond vn)))))
    let last = columns.len() - 1;
    for (index, (column, value)) in columns.iter().zip(position).enumerate() {
        builder.push("(");
        push_beyond(builder, column, &value);
        if index < last {
            builder.push(" OR (");
            push_equal(builder, column, value);
            builder.push(" AND ");
        }
    }
    builder.push(")".repeat(2 * last + 1));
    Ok(())
}

/// Redundant `c1 >= v1` (or `<=`) that lets PostgreSQL bound an index range on a `NOT NULL`
/// leading column instead of filtering every row before the position.
fn push_leading_bound(
    builder: &mut QueryBuilder<Postgres>,
    column: &KeysetColumn,
    value: &KeysetValue,
) {
    if column.nulls.is_some() || *value == KeysetValue::Null {
        return;
    }
    builder.push(column.expression).push(match column.order {
        SortOrder::Asc => " >= ",
        SortOrder::Desc => " <= ",
    });
    push_value(builder, value.clone());
    builder.push(" AND ");
}

fn push_beyond(builder: &mut QueryBuilder<Postgres>, column: &KeysetColumn, value: &KeysetValue) {
    if *value == KeysetValue::Null {
        builder.push(match column.nulls {
            Some(NullsOrder::First) => format!("{} IS NOT NULL", column.expression),
            Some(NullsOrder::Last) | None => "FALSE".to_owned(),
        });
        return;
    }

    let trailing_nulls = column.nulls == Some(NullsOrder::Last);
    if trailing_nulls {
        builder.push("(");
    }
    builder.push(column.expression).push(match column.order {
        SortOrder::Asc => " > ",
        SortOrder::Desc => " < ",
    });
    push_value(builder, value.clone());
    if trailing_nulls {
        builder
            .push(" OR ")
            .push(column.expression)
            .push(" IS NULL)");
    }
}

fn push_equal(builder: &mut QueryBuilder<Postgres>, column: &KeysetColumn, value: KeysetValue) {
    builder.push(column.expression);
    if value == KeysetValue::Null {
        builder.push(" IS NULL");
    } else {
        builder.push(" = ");
        push_value(builder, value);
    }
}

fn push_value(builder: &mut QueryBuilder<Postgres>, value: KeysetValue) {
    match value {
        KeysetValue::Null => builder.push("NULL"),
        KeysetValue::Text(value) => builder.push_bind(value),
        KeysetValue::Timestamp(value) => builder.push_bind(value),
        KeysetValue::Uuid(value) => builder.push_bind(value),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tie_breaker() -> KeysetColumn {
        KeysetColumn::required("id", SortOrder::Asc)
    }

    fn after(columns: &[KeysetColumn], position: Vec<KeysetValue>) -> String {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT * FROM t WHERE TRUE");
        if let Err(error) = push_after(&mut builder, columns, position) {
            panic!("valid keyset position rejected: {error}");
        }
        builder.sql().as_str().to_owned()
    }

    #[test]
    fn should_render_order_by_with_explicit_null_placement_only_for_nullable_columns() {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT * FROM t");
        push_order_by(
            &mut builder,
            &[
                KeysetColumn::nullable("email", SortOrder::Desc, NullsOrder::Last),
                KeysetColumn::nullable("phone", SortOrder::Asc, NullsOrder::First),
                tie_breaker(),
            ],
        );

        assert_eq!(
            "SELECT * FROM t ORDER BY email DESC NULLS LAST, phone ASC NULLS FIRST, id ASC",
            builder.sql().as_str()
        );
    }

    #[test]
    fn should_bound_and_select_rows_after_required_value_in_sort_direction() {
        let id = KeysetValue::Uuid(Uuid::nil());

        assert_eq!(
            "SELECT * FROM t WHERE TRUE AND name >= $1 AND (name > $2 OR (name = $3 AND (id > $4)))",
            after(
                &[
                    KeysetColumn::required("name", SortOrder::Asc),
                    tie_breaker()
                ],
                vec![KeysetValue::Text("a".to_owned()), id.clone()],
            )
        );
        assert_eq!(
            "SELECT * FROM t WHERE TRUE AND created <= $1 AND (created < $2 OR (created = $3 AND (id > $4)))",
            after(
                &[
                    KeysetColumn::required("created", SortOrder::Desc),
                    tie_breaker()
                ],
                vec![KeysetValue::Timestamp(OffsetDateTime::UNIX_EPOCH), id],
            )
        );
    }

    #[test]
    fn should_keep_trailing_nulls_after_a_non_null_position() {
        assert_eq!(
            "SELECT * FROM t WHERE TRUE AND ((email < $1 OR email IS NULL) OR (email = $2 AND (id > $3)))",
            after(
                &[
                    KeysetColumn::nullable("email", SortOrder::Desc, NullsOrder::Last),
                    tie_breaker(),
                ],
                vec![
                    KeysetValue::Text("b@example.test".to_owned()),
                    KeysetValue::Uuid(Uuid::nil()),
                ],
            )
        );
    }

    #[test]
    fn should_continue_within_nulls_from_a_null_position() {
        let id = KeysetValue::Uuid(Uuid::nil());

        assert_eq!(
            "SELECT * FROM t WHERE TRUE AND (FALSE OR (email IS NULL AND (id > $1)))",
            after(
                &[
                    KeysetColumn::nullable("email", SortOrder::Asc, NullsOrder::Last),
                    tie_breaker(),
                ],
                vec![KeysetValue::Null, id.clone()],
            )
        );
        assert_eq!(
            "SELECT * FROM t WHERE TRUE AND (first_name IS NOT NULL OR (first_name IS NULL AND (id > $1)))",
            after(
                &[
                    KeysetColumn::nullable("first_name", SortOrder::Desc, NullsOrder::First),
                    tie_breaker(),
                ],
                vec![KeysetValue::Null, id],
            )
        );
    }

    #[test]
    fn should_nest_every_ordering_column_before_the_tie_breaker() {
        assert_eq!(
            "SELECT * FROM t WHERE TRUE AND ((first_name > $1 OR first_name IS NULL) OR (first_name = $2 AND (FALSE OR (last_name IS NULL AND (id > $3)))))",
            after(
                &[
                    KeysetColumn::nullable("first_name", SortOrder::Asc, NullsOrder::Last),
                    KeysetColumn::nullable("last_name", SortOrder::Asc, NullsOrder::Last),
                    tie_breaker(),
                ],
                vec![
                    KeysetValue::Text("Ada".to_owned()),
                    KeysetValue::Null,
                    KeysetValue::Uuid(Uuid::nil()),
                ],
            )
        );
    }

    #[test]
    fn should_reject_position_that_does_not_match_the_ordering() {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT * FROM t WHERE TRUE");

        assert_eq!(
            Err(KeysetArityError {
                columns: 1,
                values: 2,
            }),
            push_after(
                &mut builder,
                &[tie_breaker()],
                vec![KeysetValue::Null, KeysetValue::Uuid(Uuid::nil())],
            )
        );
        assert_eq!(
            Err(KeysetArityError {
                columns: 0,
                values: 0,
            }),
            push_after(&mut builder, &[], vec![])
        );
        assert_eq!("SELECT * FROM t WHERE TRUE", builder.sql().as_str());
    }
}
