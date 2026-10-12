use crate::{
    product_listing_document::{
        ProductListingDocument, SalePricesDocument, SourcePriceDocument, TextDocument,
    },
    product_listing_image_document::ProductListingImageDocument,
};
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_price::ProductListingPrice,
};
use product_listing_service::ports::ProductListingSearchFilterMatchSource;

use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProductListingProjectionDocumentError {
    #[error("sale valuation is missing its immutable FX snapshot")]
    MissingSaleSnapshot,
    #[error("active product source must not receive a sale FX snapshot")]
    UnexpectedSaleSnapshot,
    #[error("sale valuation FX snapshot ID does not match the supplied snapshot")]
    SaleSnapshotMismatch {
        valuation_fx_rate_id: FxRateId,
        snapshot_fx_rate_id: FxRateId,
    },
    #[error("sale FX snapshot cannot convert the native source price")]
    InvalidSaleSnapshot {
        #[source]
        source: FxRateSnapshotError,
    },
}

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

pub(crate) fn product_listing_document(
    product: &ProductListingSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<ProductListingDocument, ProductListingProjectionDocumentError> {
    let (sale_prices, sale_observation_fx_rate_id, sale_observed_at) =
        sale_projection(product, sale_snapshot)?;
    let (title, language) = selected_title(product);

    Ok(ProductListingDocument {
        product_listing_id: product.product_listing_id,
        product_listing_title_slug_id: product.product_listing_title_slug_id.clone(),
        listing_source_id: product.source.listing_source_id,
        source_listing_id: product.source_listing_id.clone(),
        event_id: product.current_event_id,
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
        sale_prices,
        sale_observation_fx_rate_id,
        sale_observed_at,
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
    })
}

type SaleProjection = (
    Option<SalePricesDocument>,
    Option<FxRateId>,
    Option<OffsetDateTime>,
);

fn sale_projection(
    product: &ProductListingSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<SaleProjection, ProductListingProjectionDocumentError> {
    let observation = if product.availability == Some(ListingAvailability::SoldOut) {
        product.sale_observation
    } else {
        None
    };
    match (observation, sale_snapshot) {
        (None, None) => Ok((None, None, None)),
        (None, Some(_)) => Err(ProductListingProjectionDocumentError::UnexpectedSaleSnapshot),
        (Some(observation), None)
            if !matches!(
                product.pricing.price,
                Some(ProductListingPrice::Monetary(_))
            ) =>
        {
            Ok((
                None,
                Some(observation.fx_rate_id()),
                Some(observation.observed_at()),
            ))
        }
        (Some(_), None) => Err(ProductListingProjectionDocumentError::MissingSaleSnapshot),
        (Some(_), Some(_))
            if !matches!(
                product.pricing.price,
                Some(ProductListingPrice::Monetary(_))
            ) =>
        {
            Err(ProductListingProjectionDocumentError::UnexpectedSaleSnapshot)
        }
        (Some(observation), Some(snapshot)) if observation.fx_rate_id() != snapshot.id() => Err(
            ProductListingProjectionDocumentError::SaleSnapshotMismatch {
                valuation_fx_rate_id: observation.fx_rate_id(),
                snapshot_fx_rate_id: snapshot.id(),
            },
        ),
        (Some(observation), Some(snapshot)) => {
            let Some(ProductListingPrice::Monetary(source_price)) = product.pricing.price else {
                return Err(ProductListingProjectionDocumentError::MissingSaleSnapshot);
            };
            Ok((
                Some(sale_prices(snapshot, source_price)?),
                Some(observation.fx_rate_id()),
                Some(observation.observed_at()),
            ))
        }
    }
}

fn sale_prices(
    snapshot: &FxRateSnapshot,
    source_price: money::Price,
) -> Result<SalePricesDocument, ProductListingProjectionDocumentError> {
    let amount_in = |currency| {
        snapshot
            .convert(source_price, currency, RoundingMode::HalfUp)
            .map(|price| u64::from(price.monetary_amount))
            .map_err(|source| ProductListingProjectionDocumentError::InvalidSaleSnapshot { source })
    };

    let optional_amount_in = |currency| match amount_in(currency) {
        Ok(amount) => Ok(Some(amount)),
        Err(ProductListingProjectionDocumentError::InvalidSaleSnapshot {
            source: FxRateSnapshotError::MissingQuote(missing),
        }) if missing == currency => Ok(None),
        Err(error) => Err(error),
    };

    Ok(SalePricesDocument {
        eur: amount_in(Currency::Eur)?,
        gbp: amount_in(Currency::Gbp)?,
        usd: amount_in(Currency::Usd)?,
        aud: amount_in(Currency::Aud)?,
        cad: amount_in(Currency::Cad)?,
        nzd: amount_in(Currency::Nzd)?,
        cny: amount_in(Currency::Cny)?,
        brl: amount_in(Currency::Brl)?,
        pln: amount_in(Currency::Pln)?,
        r#try: amount_in(Currency::Try)?,
        jpy: amount_in(Currency::Jpy)?,
        czk: amount_in(Currency::Czk)?,
        rub: amount_in(Currency::Rub)?,
        aed: amount_in(Currency::Aed)?,
        sar: amount_in(Currency::Sar)?,
        hkd: amount_in(Currency::Hkd)?,
        sgd: amount_in(Currency::Sgd)?,
        chf: amount_in(Currency::Chf)?,
        zar: amount_in(Currency::Zar)?,
        sek: optional_amount_in(Currency::Sek)?,
        dkk: optional_amount_in(Currency::Dkk)?,
        nok: optional_amount_in(Currency::Nok)?,
        krw: optional_amount_in(Currency::Krw)?,
        inr: optional_amount_in(Currency::Inr)?,
        twd: optional_amount_in(Currency::Twd)?,
        huf: optional_amount_in(Currency::Huf)?,
        ron: optional_amount_in(Currency::Ron)?,
        mxn: optional_amount_in(Currency::Mxn)?,
        thb: optional_amount_in(Currency::Thb)?,
    })
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
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
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
    use serde_json::Value;
    use std::collections::HashMap;
    use strum::IntoEnumIterator;
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
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
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

        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

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
    fn should_project_sale_prices_from_the_pinned_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Eur => FX_RATE_SCALE,
                        Currency::Gbp => 850_000,
                        Currency::Usd => 1_100_000,
                        Currency::Jpy => 160_000_000,
                        _ => 1_250_000,
                    },
                )
            }),
        )?
        .into_persisted(FxRateGeneration::try_from(1)?);
        let mut product = source()?;
        let source_price = money::Price::new(12_500_u64.into(), Currency::Gbp);
        product.pricing.price = Some(source_price.into());
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        product.availability = Some(ListingAvailability::SoldOut);
        let document = product_listing_document(&product, Some(&snapshot))?;
        document.validate()?;
        for currency in Currency::iter() {
            let expected = snapshot.convert(source_price, currency, RoundingMode::HalfUp)?;
            assert_eq!(
                Some(u64::from(expected.monetary_amount)),
                document.sale_price(currency),
                "{currency:?}",
            );
        }
        Ok(())
    }

    #[test]
    fn should_preserve_raw_urls_without_view_urls_in_projection_documents()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = source()?;
        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

        assert_eq!(
            Some(&serde_json::json!(
                "https://shop.example.test/product_listings/blue-vase"
            )),
            document.get("url"),
        );
        assert!(document.get("viewUrl").is_none());
        Ok(())
    }

    #[test]
    fn should_project_sold_product_without_main_price_or_sale_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));
        product.availability = Some(ListingAvailability::SoldOut);

        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

        assert!(document.get("sourcePrice").is_none());
        assert!(document.get("salePrices").is_none());
        assert_eq!(
            product
                .sale_observation
                .map(|observation| observation.fx_rate_id().to_string()),
            document
                .get("saleObservationFxRateId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        );
        assert!(document.get("saleObservedAt").is_some());
        Ok(())
    }

    #[test]
    fn should_project_sold_on_request_product_without_sale_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.pricing.price = Some(ProductListingPrice::OnRequest);
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));
        product.availability = Some(ListingAvailability::SoldOut);

        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

        assert_eq!(
            Some(&serde_json::json!("ON_REQUEST")),
            document.pointer("/sourcePrice/type")
        );
        assert!(document.get("salePrices").is_none());
        assert!(document.get("saleObservationFxRateId").is_some());
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

        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

        assert!(document.get("saleObservationFxRateId").is_none());
        assert!(document.get("saleObservedAt").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("sourcePrice").is_some());
        Ok(())
    }

    #[test]
    fn should_preserve_legacy_sale_projections_without_inventing_rates()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = FxRateSnapshot::rehydrate(
            FxRateId::new(),
            FxRateGeneration::try_from(1)?,
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::iter()
                .take(19)
                .map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )?;
        let mut product = source()?;
        let price = money::Price::new(12300_u64.into(), Currency::Eur);
        product.pricing.price = Some(price.into());
        product.availability = Some(ListingAvailability::SoldOut);
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        let document = product_listing_document(&product, Some(&snapshot))?;
        document.validate()?;
        assert_eq!(Some(12300), document.sale_price(Currency::Usd));
        assert_eq!(None, document.sale_price(Currency::Krw));
        let persistent = serde_json::to_value(&document)?;
        assert_eq!(
            19,
            persistent["salePrices"]
                .as_object()
                .ok_or("prices must be an object")?
                .len()
        );
        let restored: ProductListingDocument = serde_json::from_value(persistent.clone())?;
        restored.validate()?;

        let mut invalid = persistent;
        invalid["salePrices"]["sek"] = serde_json::json!(12300);
        let invalid: ProductListingDocument = serde_json::from_value(invalid)?;
        assert!(invalid.validate().is_err());
        Ok(())
    }
}
