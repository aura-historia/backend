use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus};
use auction_service::ports::PublicAuctionDetails;
use listing_source_core::ListingSourceId;
use schemars::JsonSchema;
use serde::Serialize;
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicAuctionData {
    #[schemars(schema_with = "crate::wire::schema::auction_id")]
    pub auction_id: AuctionId,
    pub listing_source: PublicSourceData,
    pub name: Option<crate::values::LocalizedTextData>,
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
    pub catalogue_url: Option<url::Url>,
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
    pub view_url: Option<url::Url>,
    #[schemars(schema_with = "super::format_schema")]
    pub format: Option<&'static str>,
    pub schedule: super::AuctionScheduleResponseData,
    #[schemars(schema_with = "super::status_schema")]
    pub reported_status: Option<&'static str>,
    pub reported_lot_count: Option<u32>,
    pub visible_listing_count: u64,
}
impl TryFrom<PublicAuctionDetails> for PublicAuctionData {
    type Error = listing_source_core::ReferralUrlError;

    fn try_from(value: PublicAuctionDetails) -> Result<Self, Self::Error> {
        let view_url = value
            .catalogue_url
            .as_ref()
            .map(|url| {
                listing_source_core::outbound_url(value.source.referral_configuration.as_ref(), url)
            })
            .transpose()?;
        Ok(Self {
            auction_id: value.auction_id,
            listing_source: PublicSourceData {
                listing_source_id: value.source.listing_source_id,
                name: value.source.name.as_ref().to_owned(),
                slug_id: value.source.slug_id.to_string(),
            },
            name: value.name.map(Into::into),
            catalogue_url: value.catalogue_url,
            view_url,
            format: value.format.map(AuctionFormat::as_str),
            schedule: value.schedule.into(),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            reported_lot_count: value.reported_lot_count.map(|count| count.value()),
            visible_listing_count: value.visible_active_assigned_listing_count,
        })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicSourceData {
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    pub listing_source_id: ListingSourceId,
    pub name: String,
    pub slug_id: String,
}
