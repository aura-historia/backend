use crate::{
    object_id::try_from_uuid,
    product_listing_auction::{ProductListingAuctionParts, auction_from_parts},
    product_listing_repository::{
        images_from_json, parse_listing_availability, parse_listing_lifecycle, price_from_parts,
        product_listing_price_from_parts, sale_observation_from_parts,
    },
    query_error::admin_read_error,
};
use application::{
    error::{BoxError, box_error, static_error},
    pagination::{Cursor, CursoredResult},
};
use domain_primitives::sort::SortOrder;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use product_listing_core::{
    description::Description, listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPricing, product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
};
use product_listing_service::ports::{
    ListingSourceSummary, ProductListingStorageVersion, admin_product_listing_reader::*,
};
use sqlx::{PgPool, Postgres, QueryBuilder};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct SqlxAdminProductListingReader {
    pool: PgPool,
}
impl SqlxAdminProductListingReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// No translated text, FX conversion, personalization, assessment dependency, or lifecycle exclusion.
const SUMMARY_COLUMNS: &str = "p.product_listing_id, p.product_listing_title_slug_id, p.listing_source_id, p.source_listing_id, s.name AS source_name, s.listing_source_slug_id AS source_slug_id, p.title_text, p.title_language, p.price_kind, p.price_amount, p.price_currency, p.price_estimate_min_amount, p.price_estimate_min_currency, p.price_estimate_max_amount, p.price_estimate_max_currency, p.availability, p.lifecycle, p.auction_id, p.version, p.created, p.updated";
const FROM: &str =
    " FROM product_listings p JOIN listing_sources s ON s.listing_source_id = p.listing_source_id";

#[derive(sqlx::FromRow)]
struct SummaryRow {
    product_listing_id: Uuid,
    product_listing_title_slug_id: String,
    listing_source_id: Uuid,
    source_listing_id: String,
    source_name: String,
    source_slug_id: String,
    title_text: Option<String>,
    title_language: Option<String>,
    price_kind: Option<String>,
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    availability: Option<String>,
    lifecycle: String,
    auction_id: Option<Uuid>,
    version: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}
#[derive(sqlx::FromRow)]
struct DetailsRow {
    #[sqlx(flatten)]
    summary: SummaryRow,
    description_text: Option<String>,
    description_language: Option<String>,
    url: String,
    product_images: serde_json::Value,
    lot_number: Option<String>,
    catalogue_position: Option<i64>,
    lot_bidding_opens_at: Option<OffsetDateTime>,
    lot_scheduled_closes_at: Option<OffsetDateTime>,
    lot_reported_closed_at: Option<OffsetDateTime>,
    sale_observed_at: Option<OffsetDateTime>,
    sale_observation_fx_rate_id: Option<Uuid>,
}

#[async_trait::async_trait]
impl AdminProductListingDetailsReader for SqlxAdminProductListingReader {
    async fn find_by_id(
        &self,
        id: ProductListingId,
    ) -> Result<Option<AdminProductListingDetails>, AdminProductListingReadError> {
        self.details(id, None).await
    }

    async fn find_by_id_in_source(
        &self,
        source: ListingSourceId,
        id: ProductListingId,
    ) -> Result<Option<AdminProductListingDetails>, AdminProductListingReadError> {
        self.details(id, Some(source)).await
    }
}

impl SqlxAdminProductListingReader {
    async fn details(
        &self,
        id: ProductListingId,
        source: Option<ListingSourceId>,
    ) -> Result<Option<AdminProductListingDetails>, AdminProductListingReadError> {
        let query = format!(
            "SELECT {SUMMARY_COLUMNS}, p.description_text, p.description_language, p.url, p.product_images, p.lot_number, p.catalogue_position, p.lot_bidding_opens_at, p.lot_scheduled_closes_at, p.lot_reported_closed_at, p.sale_observed_at, p.sale_observation_fx_rate_id{FROM} WHERE p.product_listing_id = "
        );
        let mut builder = QueryBuilder::<Postgres>::new(query);
        builder.push_bind(id.as_uuid());
        if let Some(source) = source {
            builder
                .push(" AND p.listing_source_id = ")
                .push_bind(source.as_uuid());
        }
        builder
            .build_query_as::<DetailsRow>()
            .fetch_optional(&self.pool)
            .await
            .map_err(admin_read_error)?
            .map(map_details)
            .transpose()
            .map_err(invalid)
    }
}

#[async_trait::async_trait]
impl AdminProductListingSearchReader for SqlxAdminProductListingReader {
    async fn search(
        &self,
        request: &AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, AdminProductListingReadError> {
        let size = request.cursor.size;
        if !(1..=100).contains(&size) {
            return Err(invalid(static_error("invalid admin listing page size")));
        }
        let mut builder =
            QueryBuilder::<Postgres>::new(format!("SELECT {SUMMARY_COLUMNS}{FROM} WHERE TRUE"));
        let scope = &request.scope;
        if let Some(query) = &scope.query {
            builder
                .push(" AND (strpos(lower(p.title_text), lower(")
                .push_bind(query)
                .push(")) > 0 OR strpos(lower(p.source_listing_id), lower(")
                .push_bind(query)
                .push(")) > 0)");
        }
        if let Some(id) = scope.listing_source_id {
            builder
                .push(" AND p.listing_source_id = ")
                .push_bind(id.as_uuid());
        }
        if let Some(id) = &scope.source_listing_id {
            builder
                .push(" AND p.source_listing_id = ")
                .push_bind(id.as_ref());
        }
        if let Some(lifecycle) = scope.lifecycle {
            builder
                .push(" AND p.lifecycle = ")
                .push_bind(lifecycle.as_str());
        }
        if let Some(availability) = scope.availability {
            builder
                .push(" AND p.availability = ")
                .push_bind(availability.as_str());
        }
        if let Some(id) = scope.auction_id {
            builder.push(" AND p.auction_id = ").push_bind(id.as_uuid());
        }
        let column = match scope.sort {
            AdminProductListingSort::Created => "p.created",
            AdminProductListingSort::Updated => "p.updated",
        };
        let (comparison, direction) = match scope.order {
            SortOrder::Asc => (" > ", " ASC"),
            SortOrder::Desc => (" < ", " DESC"),
        };
        if let Some(after) = &request.cursor.search_after {
            builder
                .push(" AND (")
                .push(column)
                .push(", p.product_listing_id)")
                .push(comparison)
                .push("(")
                .push_bind(match scope.sort {
                    AdminProductListingSort::Created => after.created,
                    AdminProductListingSort::Updated => after.updated,
                })
                .push(", ")
                .push_bind(after.product_listing_id.as_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY ")
            .push(column)
            .push(direction)
            .push(", p.product_listing_id")
            .push(direction)
            .push(" LIMIT ")
            .push_bind(i64::try_from(size + 1).map_err(|error| invalid(box_error(error)))?);
        let rows = builder
            .build_query_as::<SummaryRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(admin_read_error)?;
        let has_more = rows.len() > size as usize;
        let items = rows
            .into_iter()
            .take(size as usize)
            .map(map_summary)
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        let search_after = if has_more {
            let last = items.last().expect("nonempty bounded listing page");
            Some(AdminProductListingSearchCursor {
                product_listing_id: last.product_listing_id,
                created: last.created,
                updated: last.updated,
                scope: scope.clone(),
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

fn map_summary(row: SummaryRow) -> Result<AdminProductListingSummary, BoxError> {
    let lifecycle = parse_listing_lifecycle(&row.lifecycle).map_err(box_error)?;
    let availability =
        parse_listing_availability(row.availability.as_deref()).map_err(box_error)?;
    if lifecycle == ListingLifecycle::Withdrawn && availability.is_some() {
        return Err(static_error("withdrawn listing has availability"));
    }
    let pricing = ProductListingPricing {
        price: product_listing_price_from_parts(
            row.price_kind,
            row.price_amount,
            row.price_currency,
        )
        .map_err(box_error)?,
        price_estimate_min: price_from_parts(
            row.price_estimate_min_amount,
            row.price_estimate_min_currency,
        )
        .map_err(box_error)?,
        price_estimate_max: price_from_parts(
            row.price_estimate_max_amount,
            row.price_estimate_max_currency,
        )
        .map_err(box_error)?,
    };
    Ok(AdminProductListingSummary {
        product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
            .map_err(box_error)?,
        product_listing_title_slug_id: ProductListingSlugId::raw(
            &row.product_listing_title_slug_id,
        )
        .map_err(|_| static_error("invalid listing slug"))?,
        source: ListingSourceSummary {
            listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                .map_err(box_error)?,
            name: ListingSourceName::try_from(row.source_name).map_err(box_error)?,
            slug_id: ListingSourceSlugId::raw(&row.source_slug_id)
                .map_err(|_| static_error("invalid source slug"))?,
        },
        source_listing_id: SourceListingId::try_from(row.source_listing_id).map_err(box_error)?,
        title: localized_title(row.title_text, row.title_language)?,
        pricing,
        availability,
        lifecycle,
        auction_id: row
            .auction_id
            .map(|id| try_from_uuid(id, "Auction ID").map_err(box_error))
            .transpose()?,
        version: ProductListingStorageVersion::try_from(row.version).map_err(box_error)?,
        created: row.created,
        updated: row.updated,
    })
}
fn map_details(row: DetailsRow) -> Result<AdminProductListingDetails, BoxError> {
    let auction = auction_from_parts(ProductListingAuctionParts {
        auction_id: row.summary.auction_id,
        lot_number: row.lot_number,
        catalogue_position: row.catalogue_position,
        lot_bidding_opens_at: row.lot_bidding_opens_at,
        lot_scheduled_closes_at: row.lot_scheduled_closes_at,
        lot_reported_closed_at: row.lot_reported_closed_at,
    })
    .map_err(box_error)?;
    let description = match (row.description_text, row.description_language) {
        (None, None) => None,
        (Some(text), Some(language)) => Some(Localized::new(
            Language::from_code(&language)
                .ok_or_else(|| static_error("invalid description language"))?,
            Description::from(text),
        )),
        _ => return Err(static_error("incomplete description")),
    };
    Ok(AdminProductListingDetails {
        summary: map_summary(row.summary)?,
        description,
        url: Url::parse(&row.url).map_err(box_error)?,
        images: images_from_json(row.product_images).map_err(box_error)?,
        auction,
        sale_observation: sale_observation_from_parts(
            row.sale_observed_at,
            row.sale_observation_fx_rate_id,
        )
        .map_err(box_error)?,
    })
}
fn invalid(source: BoxError) -> AdminProductListingReadError {
    AdminProductListingReadError::InvalidReadModel { source }
}

fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, product_listing_core::title::Title>>, BoxError> {
    match (text, language) {
        (None, None) => Ok(None),
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            Language::from_code(&language).ok_or_else(|| static_error("invalid title language"))?,
            crate::title::decode_title(&text)
                .map_err(|_| static_error("invalid persisted title"))?,
        ))),
        _ => Err(static_error("incomplete title")),
    }
}
