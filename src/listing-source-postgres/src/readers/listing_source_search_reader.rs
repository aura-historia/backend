use application::error::{box_error, static_error};
use application::pagination::{Cursor, SortKey};
use domain_primitives::object_id::ObjectIdError;
use domain_primitives::sort::{Sort, SortOrder};

use listing_source_core::{
    InvalidListingIngestionMethod, InvalidListingSourceSlug, ListingSourceId, ListingSourceName,
    ListingSourcePresentation, ListingSourceSearch, ListingSourceSlugId, PartnerizeCamref,
    PartnerizeCamrefError, PlatformTechnology, PlatformTechnologyError, ReferralConfiguration,
    SortListingSourceField,
};
use listing_source_service::ports::{
    ListingSourceSearchReadError, ListingSourceSearchReader, ListingSourceSearchReaderFactory,
};
use listing_source_service::use_cases::queries::search_listing_sources::{
    ListingSourceOperatorSummary, ListingSourceSearchCursor, ListingSourceSearchSummary,
    ListingSourceSortKey, SearchListingSourcesRequest, SearchListingSourcesResult,
};
use party_core::{
    party_id::PartyId,
    party_name::{PartyName, PartyNameError},
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use platform_postgres::SqlxTransaction;
use platform_postgres::keyset::{self, KeysetColumn, KeysetValue};
use sqlx::{Postgres, QueryBuilder};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxListingSourceSearchReaderFactory;

struct SqlxListingSourceSearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxListingSourceSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ListingSourceSearchReaderFactory<SqlxTransaction> for SqlxListingSourceSearchReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ListingSourceSearchReader + 'tx {
        SqlxListingSourceSearchReader {
            connection: tx.connection(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ListingSourceSearchRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    listing_source_name: String,
    operator_party_id: uuid::Uuid,
    operator_party_slug_id: String,
    operator_party_name: String,
    ingestion_methods: Vec<String>,
    platform_technology: Option<String>,
    url: Option<String>,
    image: Option<String>,
    referral_configuration: Option<serde_json::Value>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
enum ListingSourceSearchRowMappingError {
    #[error("invalid listing source ID persisted")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid operator Party ID persisted")]
    OperatorPartyId(#[source] ObjectIdError),
    #[error("invalid listing source slug")]
    ListingSourceSlug(#[source] InvalidListingSourceSlug),
    #[error("invalid listing source name")]
    ListingSourceName(#[source] listing_source_core::ListingSourceNameError),
    #[error("invalid listing source ingestion method")]
    IngestionMethod(#[source] InvalidListingIngestionMethod),
    #[error("invalid platform technology persisted")]
    PlatformTechnology(#[source] PlatformTechnologyError),
    #[error("invalid listing source URL")]
    Url(#[source] url::ParseError),
    #[error("invalid listing source operator Party slug")]
    PartySlug(#[source] InvalidPartySlugId),
    #[error("invalid listing source operator Party name")]
    PartyName(#[source] PartyNameError),
    #[error("invalid listing source referral configuration")]
    ReferralConfiguration,
    #[error("invalid Partnerize camref")]
    PartnerizeCamref(#[source] PartnerizeCamrefError),
}

impl TryFrom<ListingSourceSearchRow> for ListingSourceSearchSummary {
    type Error = ListingSourceSearchRowMappingError;

    fn try_from(row: ListingSourceSearchRow) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                .map_err(Self::Error::ListingSourceId)?,
            listing_source_slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id)
                .map_err(Self::Error::ListingSourceSlug)?,
            name: ListingSourceName::try_from(row.listing_source_name)
                .map_err(Self::Error::ListingSourceName)?,
            operator: ListingSourceOperatorSummary {
                party_id: PartyId::try_from(row.operator_party_id)
                    .map_err(Self::Error::OperatorPartyId)?,
                party_slug_id: PartySlugId::raw(row.operator_party_slug_id)
                    .map_err(Self::Error::PartySlug)?,
                name: PartyName::try_from(row.operator_party_name)
                    .map_err(Self::Error::PartyName)?,
            },
            platform_technology: row
                .platform_technology
                .map(PlatformTechnology::try_from)
                .transpose()
                .map_err(Self::Error::PlatformTechnology)?,
            ingestion_methods: row
                .ingestion_methods
                .into_iter()
                .map(|value| value.parse().map_err(Self::Error::IngestionMethod))
                .collect::<Result<_, _>>()?,
            presentation: ListingSourcePresentation {
                url: row
                    .url
                    .map(|value| Url::parse(&value).map_err(Self::Error::Url))
                    .transpose()?,
                image: row
                    .image
                    .map(|value| Url::parse(&value).map_err(Self::Error::Url))
                    .transpose()?,
            },
            referral_configuration: parse_referral_configuration(row.referral_configuration)?,
            created: row.created,
            updated: row.updated,
        })
    }
}

fn parse_referral_configuration(
    value: Option<serde_json::Value>,
) -> Result<Option<ReferralConfiguration>, ListingSourceSearchRowMappingError> {
    let Some(value) = value else {
        return Ok(None);
    };

    if value.get("kind").and_then(serde_json::Value::as_str) != Some("PARTNERIZE") {
        return Err(ListingSourceSearchRowMappingError::ReferralConfiguration);
    }
    let camref = value
        .get("camref")
        .and_then(serde_json::Value::as_str)
        .ok_or(ListingSourceSearchRowMappingError::ReferralConfiguration)?;

    Ok(Some(ReferralConfiguration::Partnerize {
        camref: PartnerizeCamref::try_from(camref)
            .map_err(ListingSourceSearchRowMappingError::PartnerizeCamref)?,
    }))
}

#[async_trait::async_trait]
impl ListingSourceSearchReader for SqlxListingSourceSearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchListingSourcesRequest,
    ) -> Result<SearchListingSourcesResult, ListingSourceSearchReadError> {
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| ListingSourceSearchReadError::Internal {
                source: box_error(source),
            })?;
        let limit =
            i64::try_from(size + 1).map_err(|source| ListingSourceSearchReadError::Internal {
                source: box_error(source),
            })?;
        let sort = request.effective_sort();
        let columns = keyset_columns(sort);

        let mut builder = QueryBuilder::<Postgres>::new(
            "WITH filtered AS (SELECT s.listing_source_id, s.listing_source_slug_id, s.name AS listing_source_name, s.operator_party_id, p.party_slug_id AS operator_party_slug_id, p.name AS operator_party_name, COALESCE(array_agg(m.ingestion_method) FILTER (WHERE m.ingestion_method IS NOT NULL), ARRAY[]::text[]) AS ingestion_methods, s.url, s.image, s.platform_technology, s.referral_configuration, s.created, s.updated FROM listing_sources s JOIN parties p ON p.party_id = s.operator_party_id LEFT JOIN listing_source_ingestion_methods m ON m.listing_source_id = s.listing_source_id WHERE TRUE",
        );
        push_filters(&mut builder, &request.search);
        builder.push(" GROUP BY s.listing_source_id, s.listing_source_slug_id, s.name, s.operator_party_id, p.party_slug_id, p.name, s.url, s.image, s.platform_technology, s.referral_configuration, s.created, s.updated");
        builder.push(
            ") SELECT listing_source_id, listing_source_slug_id, listing_source_name, operator_party_id, operator_party_slug_id, operator_party_name, ingestion_methods, url, image, platform_technology, referral_configuration, created, updated FROM filtered WHERE TRUE",
        );
        if let Some(after) = cursor.search_after {
            keyset::push_after(&mut builder, &columns, position(sort.sort, after)?).map_err(
                |source| ListingSourceSearchReadError::Internal {
                    source: box_error(source),
                },
            )?;
        }
        keyset::push_order_by(&mut builder, &columns);
        builder.push(" LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<ListingSourceSearchRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| ListingSourceSearchReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        let has_more = rows.len() > size_usize;
        rows.truncate(size_usize);
        let next_sort_key = rows
            .last()
            .filter(|_| has_more)
            .map(|row| row.sort_key(sort.sort));
        let items = rows
            .into_iter()
            .map(ListingSourceSearchSummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| ListingSourceSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after =
            next_sort_key
                .zip(items.last())
                .map(|(sort_key, last)| ListingSourceSearchCursor {
                    scope: request.scope_binding(),
                    sort_key,
                    id: last.listing_source_id,
                });

        Ok(SearchListingSourcesResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

impl ListingSourceSearchRow {
    /// Stored value of `field`, exactly as PostgreSQL orders it.
    fn sort_key(&self, field: SortListingSourceField) -> ListingSourceSortKey {
        match field {
            SortListingSourceField::Name => {
                ListingSourceSortKey::Name(self.listing_source_name.clone())
            }
            SortListingSourceField::Slug => {
                ListingSourceSortKey::Slug(self.listing_source_slug_id.clone())
            }
            SortListingSourceField::Created => ListingSourceSortKey::Created(self.created),
            SortListingSourceField::Updated => ListingSourceSortKey::Updated(self.updated),
        }
    }
}

fn keyset_columns(sort: Sort<SortListingSourceField>) -> [KeysetColumn; 2] {
    let column = match sort.sort {
        SortListingSourceField::Name => "listing_source_name",
        SortListingSourceField::Slug => "listing_source_slug_id",
        SortListingSourceField::Created => "created",
        SortListingSourceField::Updated => "updated",
    };
    [
        KeysetColumn::required(column, sort.order),
        KeysetColumn::required("listing_source_id", SortOrder::Asc),
    ]
}

fn position(
    field: SortListingSourceField,
    after: ListingSourceSearchCursor,
) -> Result<Vec<KeysetValue>, ListingSourceSearchReadError> {
    if after.sort_key.field() != field {
        return Err(ListingSourceSearchReadError::Internal {
            source: static_error("listing source search cursor does not match the requested sort"),
        });
    }
    let sort_value = match after.sort_key {
        ListingSourceSortKey::Name(text) | ListingSourceSortKey::Slug(text) => {
            KeysetValue::Text(text)
        }
        ListingSourceSortKey::Created(instant) | ListingSourceSortKey::Updated(instant) => {
            KeysetValue::Timestamp(instant)
        }
    };
    Ok(vec![sort_value, KeysetValue::Uuid(after.id.into_uuid())])
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, search: &ListingSourceSearch) {
    if let Some(query) = &search.query {
        builder.push(" AND (");
        push_ilike(builder, "s.name", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "s.listing_source_slug_id", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "p.name", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "p.party_slug_id", query.as_ref());
        builder.push(")");
    }
    if let Some(query) = &search.name_query {
        builder.push(" AND ");
        push_ilike(builder, "s.name", query.as_ref());
    }
    if let Some(listing_source_id) = search.listing_source_id {
        builder
            .push(" AND s.listing_source_id = ")
            .push_bind(listing_source_id.into_uuid());
    }
    if let Some(listing_source_slug_id) = &search.listing_source_slug_id {
        builder
            .push(" AND s.listing_source_slug_id = ")
            .push_bind(listing_source_slug_id.as_ref());
    }
    if let Some(operator_party_id) = search.operator_party_id {
        builder
            .push(" AND s.operator_party_id = ")
            .push_bind(operator_party_id.into_uuid());
    }
    if let Some(platform) = &search.platform_technology {
        builder
            .push(" AND s.platform_technology = ")
            .push_bind(platform.as_ref());
    }
    if let Some(ingestion_method) = search.ingestion_method {
        builder.push(" AND EXISTS (SELECT 1 FROM listing_source_ingestion_methods filter_method WHERE filter_method.listing_source_id = s.listing_source_id AND filter_method.ingestion_method = ");
        builder.push_bind(ingestion_method.as_str());
        builder.push(")");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(listing_source_id: uuid::Uuid, operator_party_id: uuid::Uuid) -> ListingSourceSearchRow {
        ListingSourceSearchRow {
            listing_source_id,
            listing_source_slug_id: "source".to_owned(),
            listing_source_name: "Source".to_owned(),
            operator_party_id,
            operator_party_slug_id: "operator".to_owned(),
            operator_party_name: "Operator".to_owned(),
            ingestion_methods: Vec::new(),
            platform_technology: None,
            url: None,
            image: None,
            referral_configuration: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn should_reject_wrong_version_listing_source_id() {
        assert!(matches!(
            ListingSourceSearchSummary::try_from(row(uuid::Uuid::new_v4(), uuid::Uuid::now_v7())),
            Err(ListingSourceSearchRowMappingError::ListingSourceId(_))
        ));
    }

    #[test]
    fn should_reject_wrong_version_operator_party_id() {
        assert!(matches!(
            ListingSourceSearchSummary::try_from(row(uuid::Uuid::now_v7(), uuid::Uuid::new_v4())),
            Err(ListingSourceSearchRowMappingError::OperatorPartyId(_))
        ));
    }

    #[test]
    fn should_escape_like_wildcards_as_literal_text() {
        assert_eq!(r"%100\%\_ready\\\\%", like_pattern("100%_ready\\\\"));
    }

    #[test]
    fn should_parse_safe_partnerize_referral_summary() {
        let result = parse_referral_configuration(Some(serde_json::json!({
            "kind": "PARTNERIZE",
            "camref": "campaign123",
        })));

        assert!(matches!(
            result,
            Ok(Some(ReferralConfiguration::Partnerize { camref }))
                if camref.as_ref() == "campaign123"
        ));
    }

    #[test]
    fn should_reject_invalid_partnerize_referral_summary() {
        for value in [
            serde_json::json!({"kind":"OTHER","camref":"campaign123"}),
            serde_json::json!({"kind":"PARTNERIZE"}),
            serde_json::json!({"kind":"PARTNERIZE","camref":"campaign/ref"}),
        ] {
            assert!(parse_referral_configuration(Some(value)).is_err());
        }
    }

    #[test]
    fn should_reject_noncanonical_persisted_platform_technology() {
        let mut invalid = row(uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        invalid.platform_technology = Some("shopify".to_owned());
        assert!(ListingSourceSearchSummary::try_from(invalid).is_err());
    }
}
