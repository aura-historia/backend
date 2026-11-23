use crate::{
    product_listing_document::{ProductListingDocument, SourcePriceDocument, TextDocument},
    product_listing_image_document::ProductListingImageDocument,
};
use localization::Language;
use product_listing_service::ports::ProductListingSearchFilterMatchSource;

use time::OffsetDateTime;

fn exact_lot_bidding_opens_at(
    auction: Option<&product_listing_core::product_listing_auction::ProductListingAuction>,
) -> Option<OffsetDateTime> {
    auction.and_then(|auction| auction.bidding_opens())
}

fn exact_lot_scheduled_closes_at(
    auction: Option<&product_listing_core::product_listing_auction::ProductListingAuction>,
) -> Option<OffsetDateTime> {
    auction.and_then(|auction| auction.scheduled_closes())
}

/// Maps a discoverable listing to its search document. Only discoverable listings are indexed,
/// so the document never carries sale valuation: an active listing is valued at current FX.
pub(crate) fn product_listing_document(
    product: &ProductListingSearchFilterMatchSource,
) -> ProductListingDocument {
    let (title, language) = selected_title(product);

    ProductListingDocument {
        product_listing_id: product.product_listing_id,
        product_listing_title_slug_id: product.product_listing_title_slug_id.clone(),
        listing_source_id: product.source.listing_source_id,
        source_listing_id: product.source_listing_id.clone(),
        event_id: product.current_event_id,
        shipping: Some((&product.shipping).into()),
        business_geography: product.geography.business.as_ref().map(Into::into),
        inventory_geography: product.geography.inventory.as_ref().map(Into::into),
        auction_id: product
            .auction
            .as_ref()
            .and_then(|auction| auction.auction_id()),
        title: TextDocument::new(title, language),
        title_de: translated_title(product, Language::De),
        title_en: translated_title(product, Language::En),
        title_fr: translated_title(product, Language::Fr),
        title_es: translated_title(product, Language::Es),
        title_it: translated_title(product, Language::It),
        source_price: product.pricing.price.map(SourcePriceDocument::from),
        availability: product.availability,
        url: product.url.clone(),
        images: product
            .images
            .iter()
            .cloned()
            .map(ProductListingImageDocument::from)
            .collect(),
        embedding: product.embedding.clone(),
        lot_bidding_opens_at: exact_lot_bidding_opens_at(product.auction.as_ref()),
        lot_scheduled_closes_at: exact_lot_scheduled_closes_at(product.auction.as_ref()),
        created: product.created,
        updated: product.updated,
    }
}

fn selected_title(product: &ProductListingSearchFilterMatchSource) -> (&str, Language) {
    product
        .product_title
        .as_ref()
        .map(|title| (title.payload.as_ref(), title.localization))
        .or_else(|| {
            product
                .titles
                .get(&Language::En)
                .map(|title| (title.as_ref(), Language::En))
        })
        .or_else(|| {
            product
                .titles
                .iter()
                .min_by_key(|(language, _)| language.as_str())
                .map(|(language, title)| (title.as_ref(), *language))
        })
        .unwrap_or(("", Language::En))
}

fn translated_title(
    product: &ProductListingSearchFilterMatchSource,
    language: Language,
) -> Option<String> {
    product
        .titles
        .get(&language)
        .map(|title| title.as_ref().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use fxrate_core::FxRateId;
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
    use money::Currency;
    use product_listing_core::{
        listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle,
        product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
        product_listing_auction::{CataloguePosition, LotNumber},
        product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
        title::Title,
    };
    use product_listing_service::ports::ListingSourceSummary;
    use product_listing_service::ports::ProductListingSearchFilterMatchSourceEventKind;
    use std::collections::HashMap;
    use url::Url;

    fn source() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let title = Title::from("Blue vase");
        let url = Url::parse("https://shop.example.test/product_listings/blue-vase")?;
        let event_id = EventId::new();
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            shipping: Default::default(),
            geography: Default::default(),
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("blue-vase-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                operator: product_listing_service::ports::ListingSourceOperatorSummary {
                    name: party_core::party_name::PartyName::try_from("Operator")
                        .unwrap_or_else(|error| panic!("valid test operator name: {error}")),
                    partner: false,
                },
            },
            source_listing_id: SourceListingId::try_from("sku-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: Some(Localized::new(Language::En, title.clone())),
            product_description: None,
            titles: HashMap::from([(Language::En, title)]),
            descriptions: HashMap::new(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_link: listing_source_core::ViewLink::non_affiliate(url),
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    #[test]
    fn should_carry_persisted_millisecond_freshness_into_the_document()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.updated = OffsetDateTime::UNIX_EPOCH
            + time::Duration::days(20_000)
            + time::Duration::milliseconds(7);

        let document = serde_json::to_value(product_listing_document(&product))?;

        // Consecutive advances differ by at least one millisecond, which `date` preserves.
        assert_eq!(
            Some(&serde_json::json!("2024-10-04T00:00:00.007Z")),
            document.get("updated"),
        );
        Ok(())
    }

    #[test]
    fn should_project_only_exact_lot_times() -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        let bidding_opens_at = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1);
        let scheduled_closes_at = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(2);
        let reported_closed_at = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(3);
        product.auction = ProductListingAuction::new(
            None,
            Some(LotNumber::try_from("Lot 12A")?),
            Some(CataloguePosition::new(12)?),
            Some(bidding_opens_at),
            Some(scheduled_closes_at),
            Some(reported_closed_at),
        )?;

        let document = serde_json::to_value(product_listing_document(&product))?;

        assert_eq!(
            Some(&serde_json::json!(
                bidding_opens_at.format(&time::format_description::well_known::Rfc3339)?
            )),
            document.get("lotBiddingOpensAt"),
        );
        assert_eq!(
            Some(&serde_json::json!(
                scheduled_closes_at.format(&time::format_description::well_known::Rfc3339)?
            )),
            document.get("lotScheduledClosesAt"),
        );
        assert!(document.get("lotReportedClosedAt").is_none());
        assert!(document.get("auctionId").is_none());
        Ok(())
    }

    #[test]
    fn should_preserve_raw_urls_without_view_links_in_projection_documents()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = source()?;
        let document = serde_json::to_value(product_listing_document(&product))?;

        assert_eq!(
            Some(&serde_json::json!(
                "https://shop.example.test/product_listings/blue-vase"
            )),
            document.get("url"),
        );
        assert!(document.get("viewUrl").is_none());
        assert!(document.get("viewLink").is_none());
        Ok(())
    }

    #[test]
    fn should_omit_sale_observation_metadata_for_active_relisted_product()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.pricing.price = Some(money::Price::new(12_500_u64.into(), Currency::Gbp).into());
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));

        let document = serde_json::to_value(product_listing_document(&product))?;

        assert!(document.get("saleObservationFxRateId").is_none());
        assert!(document.get("saleObservedAt").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("sourcePrice").is_some());
        Ok(())
    }
}
