use application::{error::box_error, pagination::Cursor};
use partnership_core::partnership_id::PartnershipId;
use party_service::{
    ports::party_location_manager_reader::{
        PartyLocationManagerReader, PartyLocationManagerReaderFactory,
    },
    use_cases::{party_locations::PartyLocationError, queries::list_party_location_managers::*},
};
use platform_postgres::SqlxTransaction;
use serde_email::Email;
use sqlx::{FromRow, Postgres, QueryBuilder};
use user_core::{first_name::FirstName, last_name::LastName, user_id::UserId};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyLocationManagerReaderFactory;

struct LocationManagerReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl PartyLocationManagerReaderFactory<SqlxTransaction> for SqlxPartyLocationManagerReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyLocationManagerReader + 'tx {
        LocationManagerReader {
            connection: tx.connection(),
        }
    }
}

#[derive(FromRow)]
struct LocationManagerRow {
    user_id: Uuid,
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
    explicit: bool,
    partnership_ids: Vec<Uuid>,
}

fn invalid(source: impl std::error::Error + Send + Sync + 'static) -> PartyLocationError {
    PartyLocationError::InvalidPersistedState {
        source: box_error(source),
    }
}

#[derive(thiserror::Error)]
#[error("invalid persisted manager email")]
struct InvalidManagerEmail(#[source] serde_email::EmailError);

impl std::fmt::Debug for InvalidManagerEmail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // EmailError contains the submitted value; retain it only for controlled diagnosis.
        formatter.write_str("InvalidManagerEmail(<redacted>)")
    }
}

impl TryFrom<LocationManagerRow> for PartyLocationManagerView {
    type Error = PartyLocationError;

    fn try_from(row: LocationManagerRow) -> Result<Self, Self::Error> {
        let mut access = Vec::with_capacity(2);
        if row.explicit {
            access.push(PartyLocationManagerAccess::Explicit);
        }
        if !row.partnership_ids.is_empty() {
            access.push(PartyLocationManagerAccess::Partnership {
                partnership_ids: row
                    .partnership_ids
                    .into_iter()
                    .map(|id| PartnershipId::try_from(id).map_err(invalid))
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(Self {
            user_id: UserId::try_from(row.user_id).map_err(invalid)?,
            email: Email::try_from(row.email.as_str())
                .map_err(|source| invalid(InvalidManagerEmail(source)))?,
            first_name: row.first_name.map(FirstName::from),
            last_name: row.last_name.map(LastName::from),
            access,
        })
    }
}

#[async_trait::async_trait]
impl PartyLocationManagerReader for LocationManagerReader<'_> {
    async fn list(
        &mut self,
        request: &ListPartyLocationManagersRequest,
    ) -> Result<PartyLocationManagersPage, PartyLocationError> {
        let size = request.cursor.size.clamp(1, 100);
        // Aggregate authority before filtering/pagination so both reasons survive an access filter.
        // Identity and authority are read in one statement snapshot, with no per-user hydration.
        let mut query = QueryBuilder::<Postgres>::new(
            "WITH authority AS (SELECT user_id, TRUE AS explicit, NULL::uuid AS partnership_id \
             FROM party_location_management_grants WHERE party_id = ",
        );
        query.push_bind(request.party_id.into_uuid());
        query.push(
            " UNION ALL SELECT m.user_id, FALSE, p.partnership_id \
                    FROM partnership_members m JOIN partnerships p USING (partnership_id) \
                    WHERE p.business_state = 'ACTIVE' AND p.party_id = ",
        );
        query.push_bind(request.party_id.into_uuid());
        query.push(
            "), managers AS (SELECT user_id, bool_or(explicit) AS explicit, \
                    COALESCE(array_agg(partnership_id ORDER BY partnership_id) \
                    FILTER (WHERE partnership_id IS NOT NULL), '{}'::uuid[]) AS partnership_ids \
                    FROM authority GROUP BY user_id) \
                    SELECT u.user_id, u.email, u.first_name, u.last_name, \
                    m.explicit, m.partnership_ids FROM managers m JOIN users u USING (user_id) \
                    WHERE NOT u.suspended",
        );
        if let Some(user_id) = request.user_id {
            query
                .push(" AND u.user_id = ")
                .push_bind(user_id.into_uuid());
        }
        if let Some(after) = request.cursor.search_after {
            query.push(" AND u.user_id > ").push_bind(after.into_uuid());
        }
        match request.access {
            Some(PartyLocationManagerAccessKind::Explicit) => {
                query.push(" AND m.explicit");
            }
            Some(PartyLocationManagerAccessKind::Partnership) => {
                query.push(" AND cardinality(m.partnership_ids) > 0");
            }
            None => {}
        }
        query
            .push(" ORDER BY u.user_id ASC LIMIT ")
            .push_bind((size + 1) as i64);
        let mut rows = query
            .build_query_as::<LocationManagerRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|source| PartyLocationError::TemporarilyUnavailable {
                source: box_error(source),
            })?;
        let has_more = rows.len() > size as usize;
        rows.truncate(size as usize);
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<PartyLocationManagerView>, _>>()?;
        let search_after = if has_more {
            items.last().map(|item| item.user_id)
        } else {
            None
        };
        Ok(PartyLocationManagersPage {
            items,
            cursor: Cursor { size, search_after },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> LocationManagerRow {
        LocationManagerRow {
            user_id: UserId::new().into_uuid(),
            email: "manager@example.test".to_owned(),
            first_name: None,
            last_name: None,
            explicit: true,
            partnership_ids: vec![],
        }
    }

    #[test]
    fn rejects_invalid_persisted_identity_with_original_email_cause_redacted() {
        let email = "private-invalid-email";
        let error = PartyLocationManagerView::try_from(LocationManagerRow {
            email: email.to_owned(),
            ..row()
        })
        .unwrap_err();
        assert!(!error.to_string().contains(email));
        assert!(!format!("{error:?}").contains(email));
        let PartyLocationError::InvalidPersistedState { source } = error else {
            panic!("expected invalid persisted state");
        };
        assert!(!source.to_string().contains(email));
        assert!(!format!("{source:?}").contains(email));
        assert!(matches!(
            source.source().unwrap().downcast_ref::<serde_email::EmailError>(),
            Some(serde_email::EmailError::Invalid { raw_email }) if raw_email == email
        ));
        for invalid_row in [
            LocationManagerRow {
                user_id: Uuid::new_v4(),
                ..row()
            },
            LocationManagerRow {
                partnership_ids: vec![Uuid::new_v4()],
                ..row()
            },
        ] {
            assert!(matches!(
                PartyLocationManagerView::try_from(invalid_row),
                Err(PartyLocationError::InvalidPersistedState { .. })
            ));
        }
    }
}
