use application::{error::box_error, pagination::Cursor};
use geo::{
    GeographicDescription, SpatialPosition,
    data::geography_data::{GeographicDescriptionData, SpatialPositionData},
};
use geo_service::resolution::{ResolutionReader, ResolutionSubject, RetentionPurpose};
use party_service::discovery::*;
use platform_postgres::SqlxTransaction;
use sqlx::{FromRow, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyLocationDiscoveryReaderFactory;
struct Reader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}
impl PartyLocationDiscoveryReaderFactory<SqlxTransaction>
    for SqlxPartyLocationDiscoveryReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyLocationDiscoveryReader + 'tx {
        Reader {
            connection: tx.connection(),
        }
    }
}
fn invalid(e: impl std::error::Error + Send + Sync + 'static) -> DiscoverPartyLocationsError {
    DiscoverPartyLocationsError::InvalidState(box_error(e))
}
fn corrupt() -> DiscoverPartyLocationsError {
    invalid(std::io::Error::other(
        "inconsistent public location geography",
    ))
}

// Predicate fields and distance all refer to this one location and current eligible candidate.
// Provider payloads remain private; accepted evidence is decoded in a single batch below.
const SELECT: &str = r#"
WITH candidates AS (
 SELECT l.party_location_id, l.party_id, p.name AS party_name, l.label,
 l.geography, l.position, l.revision, l.input_revision,
 CASE WHEN b.guard=l.revision AND b.input_revision=l.input_revision
 AND c.storage_permitted AND r.valid_until>clock_timestamp() AND NOT r.erased
 THEN r.evidence->'candidates'->COALESCE(
   CASE WHEN b.selected_generation=r.generation THEN b.selected_candidate END,
   (r.evidence->>'accepted_index')::integer)
 END AS accepted
 FROM party_locations l JOIN parties p ON p.party_id=l.party_id
 LEFT JOIN geo_resolution_bindings b ON b.subject_kind='PARTY_LOCATION' AND b.subject_id=l.party_location_id
 LEFT JOIN geo_resolutions r ON r.resolution_id=b.resolution_id
 LEFT JOIN geo_resolution_provider_controls c ON c.profile=r.profile
 WHERE l.lifecycle='ACTIVE'
), effective AS (
 SELECT candidates.*,
 COALESCE(geography->>'country',accepted->'description'->>'country') AS country,
 COALESCE(geography->>'subdivision',accepted->'description'->>'subdivision') AS subdivision,
 COALESCE(geography->>'reference_release',accepted->'description'->>'reference_release',
 '#RELEASE#') AS reference_release,
 COALESCE(position,NULLIF(accepted->'position','null'::jsonb)) AS effective_position
 FROM candidates
), points AS (
 SELECT effective.*, (effective_position->'point'->>'latitude_degrees')::double precision AS latitude,
 (effective_position->'point'->>'longitude_degrees')::double precision AS longitude,
 effective_position->>'precision' AS precision FROM effective
), distances AS (SELECT points.*,
"#;

#[derive(FromRow)]
struct Row {
    party_location_id: uuid::Uuid,
    party_id: uuid::Uuid,
    party_name: String,
    label: String,
    geography: Option<serde_json::Value>,
    position: Option<serde_json::Value>,
    revision: i64,
    input_revision: i64,
    country: Option<String>,
    subdivision: Option<String>,
    effective_position: Option<serde_json::Value>,
    distance_km: Option<f64>,
}
#[async_trait::async_trait]
impl PartyLocationDiscoveryReader for Reader<'_> {
    async fn discover(
        &mut self,
        request: &DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
        let q = &request.geography;
        let mut builder = QueryBuilder::<Postgres>::new(SELECT.replace(
            "#RELEASE#",
            geo::reference::ReferenceRelease::CURRENT.as_str(),
        ));
        if let Some(origin) = q.origin() {
            builder
                .push("2 * ")
                .push_bind(geo::core::discovery::EARTH_RADIUS_KM)
                .push(" * asin(sqrt(LEAST(1.0, GREATEST(0.0, power(sin(radians(latitude - ")
                .push_bind(origin.latitude_degrees())
                .push(")/2),2) + cos(radians(latitude))*cos(radians(")
                .push_bind(origin.latitude_degrees())
                .push("))*power(sin(radians(longitude - ")
                .push_bind(origin.longitude_degrees())
                .push(")/2),2)))))");
        } else {
            builder.push("NULL::double precision");
        }
        builder.push(" AS distance_km FROM points) SELECT party_location_id, party_id, party_name, label, geography, position, revision, input_revision, country, subdivision, effective_position, distance_km FROM distances WHERE reference_release=")
            .push_bind(q.reference_release().as_str());
        // Unresolved/empty locations cannot appear as geographic matches.
        builder.push(" AND (country IS NOT NULL OR subdivision IS NOT NULL OR effective_position IS NOT NULL)");
        if !q.countries().is_empty() {
            builder
                .push(" AND country=ANY(")
                .push_bind(
                    q.countries()
                        .iter()
                        .map(|c| c.alpha2().to_owned())
                        .collect::<Vec<_>>(),
                )
                .push(")");
        }
        if !q.subdivisions().is_empty() {
            builder
                .push(" AND subdivision=ANY(")
                .push_bind(
                    q.subdivisions()
                        .iter()
                        .map(|s| s.as_str().to_owned())
                        .collect::<Vec<_>>(),
                )
                .push(")");
        }
        if q.origin().is_some() {
            builder.push(" AND latitude BETWEEN -90 AND 90 AND longitude BETWEEN -180 AND 180 AND precision=ANY(")
                .push_bind(if q.mode()==geo::core::discovery::ProximityMode::Strict { vec!["PREMISES"] } else { vec!["PREMISES","STREET","LOCALITY","REPRESENTATIVE"] }).push(")");
            if let Some(radius) = q.radius() {
                builder
                    .push(" AND distance_km<=")
                    .push_bind(radius.to_kilometers());
            }
        }
        if let Some(cursor) = request.cursor.search_after {
            builder.push(" AND ");
            if let Some(order) = q.distance_order() {
                let distance = cursor
                    .distance_km
                    .ok_or(DiscoverPartyLocationsError::InvalidInput)?;
                builder
                    .push("(distance_km ")
                    .push(if order == geo::core::discovery::DistanceOrder::Asc {
                        ">"
                    } else {
                        "<"
                    })
                    .push_bind(distance)
                    .push(" OR (distance_km=")
                    .push_bind(distance)
                    .push(" AND party_location_id>")
                    .push_bind(cursor.location_id.into_uuid())
                    .push("))");
            } else {
                builder
                    .push("party_location_id>")
                    .push_bind(cursor.location_id.into_uuid());
            }
        }
        builder.push(" ORDER BY ");
        if let Some(order) = q.distance_order() {
            builder.push("distance_km ").push(order.as_str()).push(", ");
        }
        builder
            .push("party_location_id ASC LIMIT ")
            .push_bind(request.cursor.size as i64 + 1);
        let mut rows = builder
            .build_query_as::<Row>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|e| DiscoverPartyLocationsError::Read(box_error(e)))?;
        let more = rows.len() > request.cursor.size as usize;
        if more {
            rows.truncate(request.cursor.size as usize);
        }
        let subjects = rows
            .iter()
            .map(|r| ResolutionSubject::new(RetentionPurpose::PartyLocation, r.party_location_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        let resolutions =
            geo_postgres::SqlxResolutionRepositoryFactory::on_connection(self.connection)
                .inspect_many(&subjects)
                .await
                .map_err(invalid)?;
        if resolutions.len() != rows.len() {
            return Err(corrupt());
        }
        let mut items = Vec::with_capacity(rows.len());
        let mut after = None;
        for (row, resolution) in rows.into_iter().zip(resolutions) {
            let submitted = row
                .geography
                .map(|v| {
                    serde_json::from_value::<GeographicDescriptionData>(v)
                        .map_err(invalid)
                        .and_then(|d| {
                            Option::<GeographicDescription>::try_from(d)
                                .map_err(invalid)
                                .and_then(|d| d.ok_or_else(corrupt))
                        })
                })
                .transpose()?;
            let submitted_position = row
                .position
                .map(|v| {
                    serde_json::from_value::<SpatialPositionData>(v)
                        .map_err(invalid)
                        .and_then(|p| SpatialPosition::try_from(p).map_err(invalid))
                })
                .transpose()?;
            let resolved = resolution
                .as_ref()
                .filter(|r| {
                    r.guard as i64 == row.revision && r.input_revision as i64 == row.input_revision
                })
                .and_then(|r| r.accepted_geography());
            let country = submitted.as_ref().and_then(|d| d.country()).or_else(|| {
                resolved
                    .and_then(|r| r.description())
                    .and_then(|d| d.country())
            });
            let subdivision = submitted
                .as_ref()
                .and_then(|d| d.subdivision())
                .or_else(|| {
                    resolved
                        .and_then(|r| r.description())
                        .and_then(|d| d.subdivision())
                });
            let position = submitted_position.or_else(|| resolved.and_then(|r| r.position()));
            geo::GeographicDescription::new_in_release(
                q.reference_release(),
                None,
                country,
                subdivision,
                None,
            )
            .map_err(invalid)?;
            let party_name = party_core::party_name::PartyName::try_from(row.party_name.clone())
                .map_err(invalid)?;
            let label = party_core::party_location::PartyLocationLabel::new(row.label.clone())
                .map_err(invalid)?;
            if party_name.as_ref() != row.party_name || label.as_str() != row.label {
                return Err(corrupt());
            }
            let sql_position = row
                .effective_position
                .map(|v| {
                    serde_json::from_value::<SpatialPositionData>(v)
                        .map_err(invalid)
                        .and_then(|p| SpatialPosition::try_from(p).map_err(invalid))
                })
                .transpose()?;
            if country.map(|c| c.alpha2()) != row.country.as_deref()
                || subdivision.map(|s| s.as_str()) != row.subdivision.as_deref()
                || position != sql_position
            {
                return Err(DiscoverPartyLocationsError::InconsistentSnapshot);
            }
            if !q.matches(country, subdivision.as_ref(), position) {
                return Err(corrupt());
            }
            let location_id = row.party_location_id.try_into().map_err(invalid)?;
            after = Some(PartyLocationDiscoveryCursor {
                location_id,
                distance_km: q.distance_order().and(row.distance_km),
            });
            let derived = (submitted.as_ref().and_then(|d| d.country()).is_none()
                && country.is_some())
                || (submitted.as_ref().and_then(|d| d.subdivision()).is_none()
                    && subdivision.is_some())
                || (submitted_position.is_none() && position.is_some());
            let asserted = submitted.is_some() || submitted_position.is_some();
            items.push(PublicPartyLocation {
                party_id: row.party_id.try_into().map_err(invalid)?,
                party_name: row.party_name,
                location_id,
                label: row.label,
                country,
                subdivision,
                position,
                basis: if derived && asserted {
                    "MIXED_ASSERTED_AND_DERIVED"
                } else if derived {
                    "ACCEPTED_DERIVED"
                } else {
                    "CALLER_ASSERTED"
                },
                distance_km: row
                    .distance_km
                    .zip(position)
                    .map(|(d, p)| geo::core::discovery::display_distance_km(d, p.precision())),
            });
        }
        Ok(DiscoverPartyLocationsResult {
            items,
            cursor: Cursor {
                size: request.cursor.size,
                search_after: more.then_some(after).flatten(),
            },
            total: None,
        })
    }
}
