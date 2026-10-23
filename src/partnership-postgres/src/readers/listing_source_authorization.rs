use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use partnership_service::ports::*;
use sqlx::PgPool;
use user_core::user_id::UserId;
#[derive(Debug, thiserror::Error)]
#[error("invalid administered ListingSource ID persisted")]
struct InvalidAdministeredListingSourceId(#[source] domain_primitives::object_id::ObjectIdError);

#[derive(sqlx::FromRow)]
struct AdministeredListingSourceRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
}

impl TryFrom<AdministeredListingSourceRow> for AdministeredListingSource {
    type Error = SourceAuthorizationError;

    fn try_from(row: AdministeredListingSourceRow) -> Result<Self, Self::Error> {
        let name = ListingSourceName::try_from(row.name.as_str()).map_err(|source| {
            SourceAuthorizationError::InvalidReadModel {
                source: box_error(source),
            }
        })?;
        if name.as_ref() != row.name {
            return Err(SourceAuthorizationError::InvalidReadModel {
                source: application::error::static_error("noncanonical administered-source name"),
            });
        }
        Ok(Self {
            listing_source_id: ListingSourceId::try_from(row.listing_source_id).map_err(
                |error| SourceAuthorizationError::InvalidReadModel {
                    source: box_error(InvalidAdministeredListingSourceId(error)),
                },
            )?,
            slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|error| {
                SourceAuthorizationError::InvalidReadModel {
                    source: box_error(error),
                }
            })?,
            name,
        })
    }
}

#[derive(Clone)]
pub struct SqlxListingSourceAuthorization {
    pool: PgPool,
}
impl SqlxListingSourceAuthorization {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait::async_trait]
impl ListingSourceAuthorization for SqlxListingSourceAuthorization {
    async fn can_write_source(
        &self,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) -> Result<bool, SourceAuthorizationError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(\
                SELECT 1 \
                FROM partnership_members member \
                JOIN partnership_listing_source_grants source_grant \
                  ON source_grant.partnership_id = member.partnership_id \
                JOIN partnerships partnership \
                  ON partnership.partnership_id = source_grant.partnership_id \
                JOIN listing_sources source \
                  ON source.listing_source_id = source_grant.listing_source_id \
                WHERE member.user_id = $1 \
                  AND source_grant.listing_source_id = $2 \
                  AND partnership.party_id = source.operator_party_id \
                  AND partnership.business_state = 'ACTIVE'\
            )",
        )
        .bind(user_id.into_uuid())
        .bind(listing_source_id.into_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(|source| SourceAuthorizationError::TemporarilyUnavailable {
            source: box_error(source),
        })
    }
    async fn list_sources_user_administers(
        &self,
        user_id: UserId,
        size: u32,
        after: Option<&AdministeredListingSourcePosition>,
    ) -> Result<AdministeredListingSourcePage, SourceAuthorizationError> {
        if !(1..=100).contains(&size) {
            return Err(SourceAuthorizationError::Internal {
                source: application::error::static_error("invalid administered-source page size"),
            });
        }
        let rows = sqlx::query_as::<_, AdministeredListingSourceRow>(
            r#"SELECT s.listing_source_id, s.listing_source_slug_id, s.name
               FROM listing_sources s
               WHERE EXISTS (
                   SELECT 1 FROM partnership_members member
                   JOIN partnership_listing_source_grants source_grant
                     ON source_grant.partnership_id = member.partnership_id
                   JOIN partnerships partnership
                     ON partnership.partnership_id = member.partnership_id
                   WHERE member.user_id = $1
                     AND source_grant.listing_source_id = s.listing_source_id
                     AND partnership.party_id = s.operator_party_id
                     AND partnership.business_state = 'ACTIVE'
               )
                 AND ($2::text IS NULL OR (s.name COLLATE "C", s.listing_source_id) > ($2 COLLATE "C", $3::uuid))
               ORDER BY s.name COLLATE "C" ASC, s.listing_source_id ASC
               LIMIT $4"#,
        )
        .bind(user_id.into_uuid())
        .bind(after.map(|position| position.name.as_ref()))
        .bind(after.map(|position| position.listing_source_id.into_uuid()))
        .bind(i64::from(size) + 1)
        .fetch_all(&self.pool).await
        .map_err(|source| SourceAuthorizationError::TemporarilyUnavailable { source: box_error(source) })?;
        administered_sources_page(rows, size)
    }
}

fn administered_sources_page(
    rows: Vec<AdministeredListingSourceRow>,
    size: u32,
) -> Result<AdministeredListingSourcePage, SourceAuthorizationError> {
    let mut items = rows
        .into_iter()
        .map(AdministeredListingSource::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let next = if items.len() > size as usize {
        items.pop();
        items
            .last()
            .map(|source| AdministeredListingSourcePosition {
                name: source.name.clone(),
                listing_source_id: source.listing_source_id,
            })
    } else {
        None
    };
    Ok(AdministeredListingSourcePage { items, next })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(listing_source_id: uuid::Uuid) -> AdministeredListingSourceRow {
        AdministeredListingSourceRow {
            listing_source_id,
            listing_source_slug_id: "source".to_owned(),
            name: "Source".to_owned(),
        }
    }

    #[test]
    fn should_map_valid_uuidv7_listing_source_id() {
        assert!(AdministeredListingSource::try_from(row(uuid::Uuid::now_v7())).is_ok());
    }

    #[test]
    fn should_reject_wrong_version_listing_source_id() {
        assert!(AdministeredListingSource::try_from(row(uuid::Uuid::new_v4())).is_err());
    }
    #[test]
    fn should_reject_noncanonical_name_before_constructing_a_continuation() {
        let mut persisted = row(uuid::Uuid::now_v7());
        persisted.name = " Source ".into();
        assert!(matches!(
            AdministeredListingSource::try_from(persisted),
            Err(SourceAuthorizationError::InvalidReadModel { .. })
        ));
    }
}
