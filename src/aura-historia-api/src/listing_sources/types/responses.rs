use super::ReferralConfigurationData;
use listing_source_core::{ListingIngestionMethod, ListingSourceId, ListingSourceType};
use listing_source_service::ports::ListingSourceDetails;
use listing_source_service::use_cases::queries::search_listing_sources::{
    ListingSourceSearchSummary, SearchListingSourcesResult,
};
use partnership_core::partnership_id::PartnershipId;
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use serde::Serialize;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceData {
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
    name: String,
    #[serde(with = "crate::wire::listing_source_type")]
    listing_source_type: ListingSourceType,
    operator: OperatorPartyData,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform_technology: Option<String>,
    ingestion_methods: Vec<ListingIngestionMethodData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
    #[serde(with = "time::serde::rfc3339")]
    created: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: time::OffsetDateTime,
}

impl From<ListingSourceDetails> for ListingSourceData {
    fn from(value: ListingSourceDetails) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.slug_id.to_string(),
            name: value.name.to_string(),
            listing_source_type: value.listing_source_type,
            operator: OperatorPartyData::new(
                value.operator_party_id,
                &value.operator_slug_id,
                &value.operator_name,
                value.operator_partnership_id,
            ),
            platform_technology: value
                .platform_technology
                .map(|platform| platform.to_string()),
            ingestion_methods: methods_data(value.ingestion_methods),
            url: value.url,
            image: value.image,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceSearchCollectionData {
    pub(crate) items: Vec<ListingSourceSearchSummaryData>,
    pub(crate) size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) search_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<u64>,
}

impl From<SearchListingSourcesResult> for ListingSourceSearchCollectionData {
    fn from(value: SearchListingSourcesResult) -> Self {
        let items = value
            .items
            .into_iter()
            .map(ListingSourceSearchSummaryData::from)
            .collect::<Vec<_>>();
        Self {
            size: items.len() as u64,
            items,
            search_after: value
                .cursor
                .search_after
                .map(crate::listing_sources::search_listing_sources::encode_search_after),
            total: value.total,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceSearchSummaryData {
    pub(crate) listing_source_id: ListingSourceId,
    pub(crate) listing_source_slug_id: String,
    pub(crate) name: String,
    #[serde(with = "crate::wire::listing_source_type")]
    pub(crate) listing_source_type: ListingSourceType,
    operator: OperatorPartyData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) platform_technology: Option<String>,
    pub(crate) ingestion_methods: Vec<ListingIngestionMethodData>,
    pub(crate) presentation: ListingSourcePresentationData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) referral_configuration: Option<ReferralConfigurationData>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: time::OffsetDateTime,
}

impl From<ListingSourceSearchSummary> for ListingSourceSearchSummaryData {
    fn from(value: ListingSourceSearchSummary) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.listing_source_slug_id.to_string(),
            name: value.name.to_string(),
            listing_source_type: value.listing_source_type,
            operator: OperatorPartyData::new(
                value.operator.party_id,
                &value.operator.party_slug_id,
                &value.operator.name,
                value.operator.partnership_id,
            ),
            platform_technology: value
                .platform_technology
                .map(|platform| platform.to_string()),
            ingestion_methods: methods_data(value.ingestion_methods),
            presentation: ListingSourcePresentationData {
                url: value.presentation.url,
                image: value.presentation.image,
            },
            referral_configuration: value.referral_configuration.map(Into::into),
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourcePresentationData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<Url>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperatorPartyData {
    party_id: PartyId,
    party_slug_id: String,
    name: String,
    partner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    partnership_id: Option<PartnershipId>,
}

impl OperatorPartyData {
    fn new(
        party_id: PartyId,
        party_slug_id: &PartySlugId,
        name: &PartyName,
        active_partnership_id: Option<PartnershipId>,
    ) -> Self {
        Self {
            party_id,
            party_slug_id: party_slug_id.to_string(),
            name: name.to_string(),
            partner: active_partnership_id.is_some(),
            partnership_id: active_partnership_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceReferenceData {
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
}

impl From<(ListingSourceId, listing_source_core::ListingSourceSlugId)>
    for ListingSourceReferenceData
{
    fn from(
        (listing_source_id, slug_id): (ListingSourceId, listing_source_core::ListingSourceSlugId),
    ) -> Self {
        Self {
            listing_source_id,
            listing_source_slug_id: slug_id.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) enum ListingIngestionMethodData {
    #[serde(rename = "WEB_CRAWL")]
    WebCrawl,
    #[serde(rename = "SHOPIFY")]
    Shopify,
    #[serde(rename = "WOOCOMMERCE")]
    Woocommerce,
    #[serde(rename = "PARTNER_API")]
    PartnerApi,
}

impl From<ListingIngestionMethod> for ListingIngestionMethodData {
    fn from(value: ListingIngestionMethod) -> Self {
        match value {
            ListingIngestionMethod::WebCrawl => Self::WebCrawl,
            ListingIngestionMethod::Shopify => Self::Shopify,
            ListingIngestionMethod::Woocommerce => Self::Woocommerce,
            ListingIngestionMethod::PartnerApi => Self::PartnerApi,
        }
    }
}

fn methods_data(values: HashSet<ListingIngestionMethod>) -> Vec<ListingIngestionMethodData> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_by_key(|value| value.as_str());
    values.into_iter().map(Into::into).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_serialize_operator_partnership_only_for_partners()
    -> Result<(), Box<dyn std::error::Error>> {
        let party_id = PartyId::new();
        let slug = PartySlugId::raw("ada-antiques-gmbh")?;
        let name = PartyName::try_from("Ada Antiques GmbH")?;
        let partnership_id = PartnershipId::new();

        assert_eq!(
            serde_json::json!({
                "partyId": party_id,
                "partySlugId": "ada-antiques-gmbh",
                "name": "Ada Antiques GmbH",
                "partner": true,
                "partnershipId": partnership_id,
            }),
            serde_json::to_value(OperatorPartyData::new(
                party_id,
                &slug,
                &name,
                Some(partnership_id)
            ))?
        );
        assert_eq!(
            serde_json::json!({
                "partyId": party_id,
                "partySlugId": "ada-antiques-gmbh",
                "name": "Ada Antiques GmbH",
                "partner": false,
            }),
            serde_json::to_value(OperatorPartyData::new(party_id, &slug, &name, None))?
        );
        Ok(())
    }
}
