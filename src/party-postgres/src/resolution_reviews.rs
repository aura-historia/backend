//! Cross-Party review queue. Location rows and their resolutions are read in two batched
//! statements on the caller's transaction; geography adapters own the resolution codec.
use crate::locations::{ViewRow, sql_error};
use application::error::static_error;
use geo_service::resolution::{
    MAX_RESOLUTION_SUBJECTS, ResolutionReader, ResolutionSubject, RetentionPurpose,
};
use party_core::party_location_id::PartyLocationId;
use party_service::{
    ports::party_location_resolution_review_reader::*,
    use_cases::{
        party_location_resolution::{
            PartyLocationResolutionReview, PartyLocationResolutionReviewsPage, ResolutionAttention,
        },
        party_locations::{PartyLocationError, PartyLocationView},
    },
};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyLocationResolutionReviewReaderFactory;
impl PartyLocationResolutionReviewReaderFactory<SqlxTransaction>
    for SqlxPartyLocationResolutionReviewReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyLocationResolutionReviewReader + 'tx {
        Reader {
            connection: tx.connection(),
        }
    }
}
struct Reader<'a> {
    connection: &'a mut PgConnection,
}

#[async_trait::async_trait]
impl PartyLocationResolutionReviewReader for Reader<'_> {
    async fn list(
        &mut self,
        statuses: &[ResolutionAttention],
        after: Option<PartyLocationId>,
        limit: u32,
    ) -> Result<PartyLocationResolutionReviewsPage, PartyLocationError> {
        if statuses.is_empty() || !(1..=MAX_RESOLUTION_SUBJECTS as u32).contains(&limit) {
            return Err(PartyLocationError::InvalidInput);
        }
        let codes: Vec<_> = statuses.iter().map(|s| s.status().as_str()).collect();
        let mut q = QueryBuilder::<Postgres>::new(
            "SELECT l.party_location_id,l.party_id,l.label,l.roles,l.geography,l.position,\
             l.lifecycle,l.revision,l.input_revision,l.evidence FROM party_locations l \
             JOIN geo_resolution_bindings b ON b.subject_kind='PARTY_LOCATION' \
             AND b.subject_id=l.party_location_id \
             JOIN geo_resolutions r ON r.resolution_id=b.resolution_id \
             WHERE NOT r.erased AND r.status=ANY(",
        );
        q.push_bind(codes)
            .push(") AND b.selected_generation IS DISTINCT FROM r.generation");
        if let Some(id) = after {
            q.push(" AND l.party_location_id > ")
                .push_bind(id.into_uuid());
        }
        q.push(" ORDER BY l.party_location_id ASC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let mut locations = q
            .build_query_as::<ViewRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(sql_error)?
            .into_iter()
            .map(PartyLocationView::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let next = if locations.len() > limit as usize {
            locations.truncate(limit as usize);
            locations.last().map(|l| l.id)
        } else {
            None
        };
        let subjects = locations
            .iter()
            .map(|l| ResolutionSubject::new(RetentionPurpose::PartyLocation, l.id.into_uuid()))
            .collect::<Result<Vec<_>, _>>()?;
        // The locking read follows the provider -> resolution -> binding order, so each view
        // decodes a resolution and binding that no worker or review changes mid-read; the
        // status filter below then applies to that coherent state.
        let resolutions =
            geo_postgres::SqlxResolutionRepositoryFactory::on_connection(self.connection)
                .inspect_many(&subjects)
                .await?;
        if resolutions.len() != locations.len() {
            return Err(PartyLocationError::InvalidPersistedState {
                source: static_error("resolution batch does not match the location page"),
            });
        }
        let items = locations
            .into_iter()
            .zip(resolutions)
            .filter_map(|(location, resolution)| {
                let resolution = resolution?;
                ResolutionAttention::needed_by(&resolution)
                    .is_some_and(|a| statuses.contains(&a))
                    .then_some(PartyLocationResolutionReview {
                        location,
                        resolution,
                    })
            })
            .collect();
        Ok(PartyLocationResolutionReviewsPage { items, next })
    }
}
