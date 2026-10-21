use super::{AuctionQueryError, parse_query_object_id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueQuery {
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: localization::Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    pub currency: money::Currency,
    #[schemars(range(max = u64::MAX))]
    pub page_size: Option<u64>,
    pub search_after: Option<String>,
}
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueCursorData {
    #[schemars(schema_with = "crate::wire::schema::auction_id")]
    pub auction_id: String,
    #[schemars(range(max = u32::MAX))]
    pub catalogue_position: Option<u32>,
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    pub product_listing_id: String,
}
impl CatalogueCursorData {
    fn try_into_cursor(
        self,
    ) -> Result<product_listing_service::ports::AuctionCatalogueCursor, AuctionQueryError> {
        Ok(product_listing_service::ports::AuctionCatalogueCursor {
            auction_id: parse_query_object_id(
                &self.auction_id,
                "searchAfter.auctionId",
                "Auction",
            )?,
            catalogue_position: self.catalogue_position,
            product_listing_id: parse_query_object_id(
                &self.product_listing_id,
                "searchAfter.productListingId",
                "ProductListing",
            )?,
        })
    }
}

pub fn parse_catalogue_cursor(
    value: String,
) -> Result<product_listing_service::ports::AuctionCatalogueCursor, AuctionQueryError> {
    serde_json::from_str::<CatalogueCursorData>(&value)
        .map_err(AuctionQueryError::InvalidCursor)?
        .try_into_cursor()
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueData {
    pub items: Vec<crate::product_listings::product_data::PersonalizedProductListingDetailsData>,
    #[schemars(range(max = u64::MAX))]
    pub page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_after: Option<CatalogueCursorData>,
}
impl
    From<
        application::pagination::CursoredResult<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            product_listing_service::ports::AuctionCatalogueCursor,
        >,
    > for CatalogueData
{
    fn from(
        value: application::pagination::CursoredResult<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            product_listing_service::ports::AuctionCatalogueCursor,
        >,
    ) -> Self {
        Self {
            page_size: value.cursor.size,
            search_after: value.cursor.search_after.map(|cursor| CatalogueCursorData {
                auction_id: cursor.auction_id.to_string(),
                catalogue_position: cursor.catalogue_position,
                product_listing_id: cursor.product_listing_id.to_string(),
            }),
            items: value
                .items
                .into_iter()
                .map(crate::product_listings::product_data::personalized_product_details_data)
                .collect(),
        }
    }
}

impl CatalogueQuery {
    pub fn try_into_request(
        self,
        auction_id: auction_core::AuctionId,
    ) -> Result<product_listing_service::use_cases::GetAuctionCatalogueRequest, AuctionQueryError>
    {
        Ok(
            product_listing_service::use_cases::GetAuctionCatalogueRequest {
                auction_id,
                language: self.language,
                currency: self.currency,
                cursor: Some(application::pagination::Cursor {
                    size: self.page_size.unwrap_or(21),
                    search_after: self.search_after.map(parse_catalogue_cursor).transpose()?,
                }),
            },
        )
    }
}
