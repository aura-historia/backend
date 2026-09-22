use crate::mapping::{APPLICATION_COLUMNS, ApplicationRow, admin_summary, view};
use application::error::box_error;
use application::pagination::{Cursor, CursoredResult};
use domain_primitives::sort::{Sort, SortOrder};
use partnership_core::{
    partnership_application_search::PartnershipApplicationSearch,
    sort_partnership_application_field::SortPartnershipApplicationField,
};
use partnership_service::ports::{
    PartnershipApplicationReadError, PartnershipApplicationReader,
    PartnershipApplicationReaderFactory, PartnershipApplicationView,
};
use partnership_service::use_cases::queries::list_admin_partnership_applications::{
    AdminPartnershipApplicationSummary, ListAdminPartnershipApplicationsRequest,
    ListAdminPartnershipApplicationsResult, PartnershipApplicationSearchCursor,
};
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, Postgres, QueryBuilder};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipApplicationReaderFactory;

struct Reader<'a> {
    connection: &'a mut sqlx::PgConnection,
}

impl SqlxPartnershipApplicationReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipApplicationReaderFactory<SqlxTransaction>
    for SqlxPartnershipApplicationReaderFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl PartnershipApplicationReader + 'a {
        Reader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnershipApplicationReader for Reader<'_> {
    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError> {
        let query = format!(
            "SELECT {APPLICATION_COLUMNS} FROM partnership_applications WHERE applicant_user_id=$1 ORDER BY created DESC, partnership_application_id DESC"
        );
        let rows = sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(query))
            .bind(user_id.into_uuid())
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipApplicationReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;
        rows.into_iter()
            .map(view)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartnershipApplicationReadError::InvalidReadModel {
                source: box_error(source),
            })
    }

    async fn search_admin(
        &mut self,
        request: &ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, PartnershipApplicationReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| PartnershipApplicationReadError::Internal {
                source: box_error(source),
            })?;
        let limit = i64::try_from(size + 1).map_err(|source| {
            PartnershipApplicationReadError::Internal {
                source: box_error(source),
            }
        })?;
        let sort = request.sort.unwrap_or(Sort {
            sort: SortPartnershipApplicationField::Created,
            order: SortOrder::Desc,
        });
        let sort_column = match sort.sort {
            SortPartnershipApplicationField::Created => "created",
            SortPartnershipApplicationField::Updated => "updated",
        };
        let order = match sort.order {
            SortOrder::Asc => "ASC",
            SortOrder::Desc => "DESC",
        };

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(APPLICATION_COLUMNS)
            .push(" FROM partnership_applications WHERE TRUE");
        push_filters(&mut builder, &request.search);
        if let Some(search_after) = cursor.search_after {
            let comparison = match sort.order {
                SortOrder::Asc => ">",
                SortOrder::Desc => "<",
            };
            builder
                .push(" AND (")
                .push(sort_column)
                .push(", partnership_application_id) ")
                .push(comparison)
                .push(" (")
                .push_bind(search_after.position)
                .push(", ")
                .push_bind(search_after.application_id.into_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY ")
            .push(sort_column)
            .push(" ")
            .push(order)
            .push(", partnership_application_id ")
            .push(order)
            .push(" LIMIT ")
            .push_bind(limit);

        let mut rows = builder
            .build_query_as::<ApplicationRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipApplicationReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;
        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items = rows
            .into_iter()
            .map(admin_summary)
            .collect::<Result<Vec<AdminPartnershipApplicationSummary>, _>>()
            .map_err(|source| PartnershipApplicationReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after = if has_more {
            items.last().map(|item| PartnershipApplicationSearchCursor {
                position: match sort.sort {
                    SortPartnershipApplicationField::Created => item.created,
                    SortPartnershipApplicationField::Updated => item.updated,
                },
                application_id: item.id,
            })
        } else {
            None
        };

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, search: &PartnershipApplicationSearch) {
    if !search.state_query.is_empty() {
        let states = search
            .state_query
            .iter()
            .copied()
            .map(|state| state.as_str().to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND business_state = ANY(")
            .push_bind(states)
            .push(")");
    }
    if let Some(applicant_user_id) = search.applicant_user_id {
        builder
            .push(" AND applicant_user_id = ")
            .push_bind(applicant_user_id.into_uuid());
    }
    if !search.proposal_type_query.is_empty() {
        let proposal_types = search
            .proposal_type_query
            .iter()
            .copied()
            .map(|proposal_type| proposal_type.as_str().to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND proposal->>'type' = ANY(")
            .push_bind(proposal_types)
            .push(")");
    }
    if let Some(listing_source_id) = search.listing_source_id {
        let listing_source_uuid = listing_source_id.into_uuid();
        builder
            .push(" AND (approved_listing_source_id = ")
            .push_bind(listing_source_uuid)
            .push(" OR (proposal->>'type' = 'EXISTING_LISTING_SOURCE' AND proposal->>'listing_source_id' = ")
            .push_bind(listing_source_uuid.to_string())
            .push("))");
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
