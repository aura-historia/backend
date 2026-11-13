use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use notification_service::ports::{
    notification_delivery_repository::NotificationDeliveryError,
    notification_list_reader::NotificationListReadError,
};
use std::collections::HashSet;

/// The given ListingSources whose operator Party holds an ACTIVE Partnership now.
///
/// Notification snapshots never store partner status, so readers resolve it here
/// in one query per page or delivery. Sources that no longer exist are not partners.
pub(crate) async fn partner_listing_source_ids<'e, E>(
    executor: E,
    listing_source_ids: impl IntoIterator<Item = ListingSourceId>,
) -> Result<HashSet<ListingSourceId>, ListingSourcePartnersError>
where
    E: sqlx::PgExecutor<'e>,
{
    let mut listing_source_ids = listing_source_ids
        .into_iter()
        .map(ListingSourceId::into_uuid)
        .collect::<Vec<_>>();
    listing_source_ids.sort_unstable();
    listing_source_ids.dedup();
    if listing_source_ids.is_empty() {
        return Ok(HashSet::new());
    }
    sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT s.listing_source_id FROM listing_sources s JOIN partnerships operator_partnership ON operator_partnership.party_id = s.operator_party_id AND operator_partnership.business_state = 'ACTIVE' WHERE s.listing_source_id = ANY($1)",
    )
    .bind(&listing_source_ids)
    .fetch_all(executor)
    .await
    .map_err(|source| ListingSourcePartnersError::ReadFailed {
        source: box_error(source),
    })?
    .into_iter()
    .map(|id| {
        ListingSourceId::try_from(id).map_err(|source| ListingSourcePartnersError::InvalidId {
            source: box_error(source),
        })
    })
    .collect()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ListingSourcePartnersError {
    #[error("listing source partner status read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted listing source ID is invalid")]
    InvalidId {
        #[source]
        source: BoxError,
    },
}

impl From<ListingSourcePartnersError> for NotificationListReadError {
    fn from(error: ListingSourcePartnersError) -> Self {
        match error {
            ListingSourcePartnersError::ReadFailed { .. } => Self::ReadFailed {
                source: box_error(error),
            },
            ListingSourcePartnersError::InvalidId { .. } => Self::InvalidReadModel {
                source: box_error(error),
            },
        }
    }
}

impl From<ListingSourcePartnersError> for NotificationDeliveryError {
    fn from(error: ListingSourcePartnersError) -> Self {
        match error {
            ListingSourcePartnersError::ReadFailed { .. } => Self::OperationFailed {
                source: box_error(error),
            },
            ListingSourcePartnersError::InvalidId { .. } => Self::InvalidPersistedState {
                source: box_error(error),
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    /// Seeds a ListingSource whose operator holds a Partnership in `partnership_state`, or none.
    pub(crate) async fn seed_listing_source(
        pool: &sqlx::PgPool,
        partnership_state: Option<&str>,
    ) -> ListingSourceId {
        let listing_source_id = ListingSourceId::new();
        let party_id = uuid::Uuid::now_v7();
        let slug = format!("source-{}", listing_source_id.as_uuid().simple());
        sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Operator')",
        )
        .bind(party_id)
        .bind(&slug)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, 'Source', $3, 'DEALER')")
            .bind(listing_source_id.into_uuid())
            .bind(&slug)
            .bind(party_id)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("seed listing source: {error}"));
        if let Some(state) = partnership_state {
            sqlx::query("INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)")
                .bind(uuid::Uuid::now_v7())
                .bind(party_id)
                .bind(state)
                .execute(pool)
                .await
                .unwrap_or_else(|error| panic!("seed partnership: {error}"));
        }
        listing_source_id
    }

    pub(crate) async fn dissolve_partnership(
        pool: &sqlx::PgPool,
        listing_source_id: ListingSourceId,
    ) {
        sqlx::query("UPDATE partnerships SET business_state = 'DISSOLVED' WHERE party_id = (SELECT operator_party_id FROM listing_sources WHERE listing_source_id = $1)")
            .bind(listing_source_id.into_uuid())
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("dissolve partnership: {error}"));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
    async fn should_return_only_sources_whose_operator_holds_an_active_partnership() {
        let pool = get_postgres_client().await;
        let active = seed_listing_source(&pool, Some("ACTIVE")).await;
        let dissolved = seed_listing_source(&pool, Some("DISSOLVED")).await;
        let none = seed_listing_source(&pool, None).await;
        let missing = ListingSourceId::new();

        let partners =
            partner_listing_source_ids(&pool, [active, dissolved, none, missing, active])
                .await
                .unwrap_or_else(|error| panic!("read partner status: {error}"));

        assert_eq!(HashSet::from([active]), partners);
    }
}
