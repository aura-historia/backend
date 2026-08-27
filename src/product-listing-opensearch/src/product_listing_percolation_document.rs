use crate::{
    product_listing_document::{
        ProductListingDocument, SalePricesDocument, SourcePriceDocument, TextDocument, continent,
        listing_availability, shop_type,
    },
    product_listing_image_document::ProductListingImageDocument,
};
use domain_primitives::event_id::EventId;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use geo::core::continent::Continent;
use indexmap::IndexSet;
use isocountry::CountryCode;
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId, source_listing_id::ShopListingId,
};
use product_listing_service::ports::{
    ProductListingPercolationInput, ProductListingPricesByCurrency,
    ProductListingSearchFilterMatchSource,
};
use serde::Serialize;
use serde_json::Value;
use shop_core::seller_slug_id::SellerSlugId;
use shop_core::shop_id::ShopId;
use shop_core::shop_slug_id::ShopSlugId;
use shop_core::shop_type::ShopType;
use time::OffsetDateTime;
use url::Url;

/// Closed-world prices for one temporary ProductListing percolation document.
///
/// Iteration 6B fills these values from one event-time persisted FX snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProductListingPercolationPricesDocument {
    eur: u64,
    gbp: u64,
    usd: u64,
    aud: u64,
    cad: u64,
    nzd: u64,
    cny: u64,
    brl: u64,
    pln: u64,
    r#try: u64,
    jpy: u64,
    czk: u64,
    rub: u64,
    aed: u64,
    sar: u64,
    hkd: u64,
    sgd: u64,
    chf: u64,
}

/// Private temporary ProductListing representation used only as a percolator input.
///
/// It deliberately does not reuse the persistent ProductListing OpenSearch document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPercolationDocument {
    product_listing_id: ProductListingId,
    product_listing_slug_id: ProductListingSlugId,
    shop_slug_id: ShopSlugId,
    seller_slug_id: SellerSlugId,
    event_id: EventId,
    shop_id: ShopId,
    seller_id: ShopId,
    shop_listing_id: ShopListingId,
    shop_name: String,
    seller_name: String,
    #[serde(with = "shop_type")]
    shop_type: ShopType,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address_addressline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address_addressline_extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address_postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address_country: Option<CountryCode>,
    #[serde(with = "continent::option", skip_serializing_if = "Option::is_none")]
    structured_address_continent: Option<Continent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<String>,
    title: TextDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_by_currency: Option<ProductListingPercolationPricesDocument>,
    #[serde(
        with = "listing_availability::option",
        skip_serializing_if = "Option::is_none"
    )]
    availability: Option<ListingAvailability>,
    url: Url,
    view_url: Url,
    #[serde(skip_serializing_if = "IndexSet::is_empty")]
    images: IndexSet<ProductListingImageDocument>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    auction_end: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingPercolationDocumentError {
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
    #[error("product percolation document serialization failed")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

/// Builds the private temporary ProductListing JSON consumed by saved-filter percolation.
///
/// The application owns valuation selection and checked conversion before this
/// adapter maps the closed-world prices to OpenSearch JSON.
pub fn product_listing_percolation_document(
    input: &ProductListingPercolationInput,
) -> Result<Value, ProductListingPercolationDocumentError> {
    serde_json::to_value(build_product_listing_percolation_document(input))
        .map_err(|source| ProductListingPercolationDocumentError::Serialize { source })
}

fn build_product_listing_percolation_document(
    input: &ProductListingPercolationInput,
) -> ProductListingPercolationDocument {
    let product = &input.source;
    let (title, language) = selected_title(product);
    let structured_address = product.address.structured.as_ref();

    ProductListingPercolationDocument {
        product_listing_id: product.product_listing_id,
        product_listing_slug_id: product.product_listing_slug_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        seller_slug_id: product.seller_slug_id.clone(),
        event_id: product.current_event_id,
        shop_id: product.shop_id,
        seller_id: product.seller_id,
        shop_listing_id: product.shop_listing_id.clone(),
        shop_name: product.shop_name.to_string(),
        seller_name: product.seller_name.to_string(),
        shop_type: product.shop_type,
        structured_address_addressline: structured_address
            .and_then(|address| address.addressline.clone()),
        structured_address_addressline_extra: structured_address
            .and_then(|address| address.addressline_extra.clone()),
        structured_address_locality: structured_address
            .and_then(|address| address.locality.clone()),
        structured_address_region: structured_address.and_then(|address| address.region.clone()),
        structured_address_postal_code: structured_address
            .and_then(|address| address.postal_code.clone()),
        structured_address_country: structured_address.and_then(|address| address.country),
        structured_address_continent: structured_address.and_then(|address| address.continent),
        geo_address: product
            .address
            .geo
            .as_ref()
            .map(|geo| format!("{},{}", geo.lat, geo.lon)),
        title: TextDocument::new(title, language),
        title_de: translated_title(product, Language::De),
        title_en: translated_title(product, Language::En),
        title_fr: translated_title(product, Language::Fr),
        title_es: translated_title(product, Language::Es),
        title_it: translated_title(product, Language::It),
        price_by_currency: input
            .valuation
            .as_ref()
            .map(|valuation| percolation_prices(valuation.prices)),
        availability: product.availability,
        url: product.url.clone(),
        view_url: product.view_url.clone(),
        images: product
            .images
            .iter()
            .cloned()
            .map(ProductListingImageDocument::from)
            .collect(),
        auction_start: product.auction.start,
        auction_end: product.auction.end,
        created: product.created,
        updated: product.updated,
    }
}

fn percolation_prices(
    prices: ProductListingPricesByCurrency,
) -> ProductListingPercolationPricesDocument {
    ProductListingPercolationPricesDocument {
        eur: prices.amount_in(Currency::Eur),
        gbp: prices.amount_in(Currency::Gbp),
        usd: prices.amount_in(Currency::Usd),
        aud: prices.amount_in(Currency::Aud),
        cad: prices.amount_in(Currency::Cad),
        nzd: prices.amount_in(Currency::Nzd),
        cny: prices.amount_in(Currency::Cny),
        brl: prices.amount_in(Currency::Brl),
        pln: prices.amount_in(Currency::Pln),
        r#try: prices.amount_in(Currency::Try),
        jpy: prices.amount_in(Currency::Jpy),
        czk: prices.amount_in(Currency::Czk),
        rub: prices.amount_in(Currency::Rub),
        aed: prices.amount_in(Currency::Aed),
        sar: prices.amount_in(Currency::Sar),
        hkd: prices.amount_in(Currency::Hkd),
        sgd: prices.amount_in(Currency::Sgd),
        chf: prices.amount_in(Currency::Chf),
    }
}

pub(crate) fn product_listing_document(
    product: &ProductListingSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<ProductListingDocument, ProductListingPercolationDocumentError> {
    let (sale_prices, sale_observation_fx_rate_id, sale_observed_at) =
        sale_projection(product, sale_snapshot)?;
    let (title, language) = selected_title(product);
    let structured_address = product.address.structured.as_ref();

    Ok(ProductListingDocument {
        product_listing_id: product.product_listing_id,
        product_listing_slug_id: product.product_listing_slug_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        seller_slug_id: product.seller_slug_id.clone(),
        event_id: product.current_event_id,
        shop_id: product.shop_id,
        seller_id: product.seller_id,
        shop_listing_id: product.shop_listing_id.clone(),
        shop_name: product.shop_name.to_string(),
        seller_name: product.seller_name.to_string(),
        shop_type: product.shop_type,
        structured_address_addressline: structured_address
            .and_then(|address| address.addressline.clone()),
        structured_address_addressline_extra: structured_address
            .and_then(|address| address.addressline_extra.clone()),
        structured_address_locality: structured_address
            .and_then(|address| address.locality.clone()),
        structured_address_region: structured_address.and_then(|address| address.region.clone()),
        structured_address_postal_code: structured_address
            .and_then(|address| address.postal_code.clone()),
        structured_address_country: structured_address.and_then(|address| address.country),
        structured_address_continent: structured_address.and_then(|address| address.continent),
        geo_address: product
            .address
            .geo
            .as_ref()
            .map(|geo| format!("{},{}", geo.lat, geo.lon)),
        title: TextDocument::new(title, language),
        title_de: translated_title(product, Language::De),
        title_en: translated_title(product, Language::En),
        title_fr: translated_title(product, Language::Fr),
        title_es: translated_title(product, Language::Es),
        title_it: translated_title(product, Language::It),
        source_price: product.pricing.price.map(|price| SourcePriceDocument {
            amount: price.monetary_amount.into(),
            currency: price.currency,
        }),
        sale_prices,
        sale_observation_fx_rate_id,
        sale_observed_at,
        availability: product.availability,
        url: product.url.clone(),
        view_url: product.view_url.clone(),
        images: product
            .images
            .iter()
            .cloned()
            .map(ProductListingImageDocument::from)
            .collect(),
        embedding: product.embedding.clone(),
        auction_start: product.auction.start,
        auction_end: product.auction.end,
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
) -> Result<SaleProjection, ProductListingPercolationDocumentError> {
    let observation = if product.availability == Some(ListingAvailability::SoldOut) {
        product.sale_observation
    } else {
        None
    };
    match (observation, sale_snapshot) {
        (None, None) => Ok((None, None, None)),
        (None, Some(_)) => Err(ProductListingPercolationDocumentError::UnexpectedSaleSnapshot),
        (Some(observation), None) if product.pricing.price.is_none() => Ok((
            None,
            Some(observation.fx_rate_id()),
            Some(observation.observed_at()),
        )),
        (Some(_), None) => Err(ProductListingPercolationDocumentError::MissingSaleSnapshot),
        (Some(observation), Some(snapshot)) if observation.fx_rate_id() != snapshot.id() => Err(
            ProductListingPercolationDocumentError::SaleSnapshotMismatch {
                valuation_fx_rate_id: observation.fx_rate_id(),
                snapshot_fx_rate_id: snapshot.id(),
            },
        ),
        (Some(observation), Some(snapshot)) => Ok((
            product
                .pricing
                .price
                .map(|price| sale_prices(snapshot, price))
                .transpose()?,
            Some(observation.fx_rate_id()),
            Some(observation.observed_at()),
        )),
    }
}

fn sale_prices(
    snapshot: &FxRateSnapshot,
    source_price: money::Price,
) -> Result<SalePricesDocument, ProductListingPercolationDocumentError> {
    let amount_in = |currency| {
        snapshot
            .convert(source_price, currency, RoundingMode::HalfUp)
            .map(|price| u64::from(price.monetary_amount))
            .map_err(
                |source| ProductListingPercolationDocumentError::InvalidSaleSnapshot { source },
            )
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
    use domain_primitives::query::range_query::RangeQuery;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use localization::Localized;
    use product_listing_core::product_listing_search::ProductListingSearch;
    use product_listing_core::{
        listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle,
        product_listing::{
            ListingSaleObservation, ProductListingAddress, ProductListingAuction,
            ProductListingPriceValuationBasis, ProductListingPricing,
        },
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::ShopListingId,
        title::Title,
    };
    use product_listing_service::ports::{
        ProductListingPercolationValuation, ProductListingPriceFilterPlan,
        ProductListingPricesByCurrency, ProductListingSearchFilterMatchSourceEventKind,
    };
    use shop_core::shop_id::ShopId;
    use shop_core::shop_name::ShopName;
    use shop_core::shop_slug_id::ShopSlugId;
    use shop_core::shop_type::ShopType;
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
            product_listing_id: ProductListingId::new(),
            product_listing_slug_id: ProductListingSlugId::from("blue-vase"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: SellerSlugId::from("seller"),
            seller_name: ShopName::from("Seller"),
            shop_listing_id: ShopListingId::from("sku-1"),
            address: ProductListingAddress::default(),
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
            auction: ProductListingAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn snapshot() -> Result<FxRateSnapshot, FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
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
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?)))
    }

    fn boundary_amounts(lower: u64, upper: Option<u64>) -> Vec<u64> {
        let mut amounts = vec![
            0,
            1,
            lower.saturating_sub(1),
            lower,
            lower.saturating_add(1),
        ];
        if let Some(upper) = upper {
            amounts.extend([upper.saturating_sub(1), upper, upper.saturating_add(1)]);
        }
        amounts.sort_unstable();
        amounts.dedup();
        amounts
    }

    fn matches_inclusive_range(amount: u64, bounds: &Value) -> bool {
        bounds
            .get("gte")
            .and_then(Value::as_u64)
            .is_none_or(|minimum| amount >= minimum)
            && bounds
                .get("lte")
                .and_then(Value::as_u64)
                .is_none_or(|maximum| amount <= maximum)
    }

    fn normal_search_membership(
        price_clause: &Value,
        source_currency: Currency,
        source_amount: u64,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let source_currency = serde_json::json!(source_currency.as_str());
        let active_ranges = price_clause
            .pointer("/bool/should")
            .and_then(Value::as_array)
            .ok_or("normal ProductListing price query has no branches")?;
        let bounds = active_ranges.iter().find_map(|branch| {
            (branch.pointer("/bool/filter/0/bool/should")?.as_array()?)
                .iter()
                .find_map(|range| {
                    (range.pointer("/bool/filter/0/term/sourcePrice.currency")
                        == Some(&source_currency))
                    .then(|| range.pointer("/bool/filter/1/range/sourcePrice.amount"))
                    .flatten()
                })
        });

        Ok(bounds.is_some_and(|bounds| matches_inclusive_range(source_amount, bounds)))
    }

    fn saved_filter_percolation_membership(
        product_listing_percolator_query: &Value,
        product_listing_percolation_document: &Value,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let filters = product_listing_percolator_query
            .pointer("/bool/filter")
            .and_then(Value::as_array)
            .ok_or("saved-filter percolator query has no filters")?;
        let (field, bounds) = filters
            .iter()
            .find_map(|filter| {
                filter
                    .get("range")
                    .and_then(Value::as_object)
                    .and_then(|ranges| {
                        ranges.iter().find_map(|(field, bounds)| {
                            field
                                .strip_prefix("priceByCurrency.")
                                .map(|field| (field, bounds))
                        })
                    })
            })
            .ok_or("saved-filter percolator query has no price range")?;
        let amount = product_listing_percolation_document
            .get("priceByCurrency")
            .and_then(Value::as_object)
            .and_then(|prices| prices.get(field))
            .and_then(Value::as_u64)
            .ok_or("percolation document has no mapped target price")?;

        Ok(matches_inclusive_range(amount, bounds))
    }

    #[test]
    fn should_use_the_private_percolation_shape_without_persistent_price_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = product_listing_percolation_document(&ProductListingPercolationInput {
            source: source()?,
            valuation: None,
        })?;

        assert!(document.get("priceByCurrency").is_none());
        assert!(document.get("sourcePrice").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("priceEstimateMin").is_none());
        assert!(document.get("priceEstimateMax").is_none());
        Ok(())
    }

    #[test]
    fn should_use_identical_sale_snapshot_values_for_persistent_and_temporary_prices()
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
        product.pricing.price = Some(source_price);
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        product.availability = Some(ListingAvailability::SoldOut);
        let prices = ProductListingPricesByCurrency::convert_all(&snapshot, source_price)?;

        let persistent =
            serde_json::to_value(product_listing_document(&product, Some(&snapshot))?)?;
        let temporary = product_listing_percolation_document(&ProductListingPercolationInput {
            source: product,
            valuation: Some(ProductListingPercolationValuation {
                basis: ProductListingPriceValuationBasis::SaleObservation,
                fx_rate_id: snapshot.id(),
                effective_at: snapshot.captured_at(),
                prices,
            }),
        })?;

        assert_eq!(
            persistent.get("salePrices"),
            temporary.get("priceByCurrency"),
        );
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
    fn should_omit_sale_observation_metadata_for_active_relisted_product()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.pricing.price = Some(money::Price::new(12_500_u64.into(), Currency::Gbp));
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
    fn should_match_normal_search_and_saved_filter_percolation_for_all_price_pairs_and_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let ranges = [
            RangeQuery {
                min: None,
                max: Some(5_u64.into()),
            },
            RangeQuery {
                min: Some(3_u64.into()),
                max: None,
            },
            RangeQuery {
                min: Some(3_u64.into()),
                max: Some(12_u64.into()),
            },
            RangeQuery {
                min: Some(0_u64.into()),
                max: Some(1_u64.into()),
            },
        ];
        let mut covered_pairs = 0;
        let mut covered_jpy_pairs = 0;

        for source_currency in Currency::iter() {
            for target_currency in Currency::iter() {
                covered_pairs += 1;
                if source_currency == Currency::Jpy || target_currency == Currency::Jpy {
                    covered_jpy_pairs += 1;
                }
                for range in ranges {
                    let price_filter = ProductListingPriceFilterPlan::compile(
                        snapshot.clone(),
                        target_currency,
                        Some(range),
                    )?;
                    let normal_price_clause =
                        crate::product_listing_search_reader::build_product_index_price_clause(
                            &price_filter,
                        )
                        .ok_or("normal ProductListing price clause missing")?;
                    let saved_filter = ProductListingSearch::new(Language::En, target_currency)
                        .with_price_query(range);
                    let product_listing_percolator_query =
                        crate::build_percolator_query(&saved_filter)?;
                    let native_range = price_filter
                        .active_native_ranges
                        .iter()
                        .find(|native| native.source_currency == source_currency)
                        .ok_or("normal ProductListing price query misses a source currency")?;

                    for source_amount in boundary_amounts(native_range.lower, native_range.upper) {
                        let source_price = money::Price::new(source_amount.into(), source_currency);
                        let prices =
                            ProductListingPricesByCurrency::convert_all(&snapshot, source_price)?;
                        let mut product = source()?;
                        product.pricing.price = Some(source_price);
                        let product_listing_percolation_document =
                            product_listing_percolation_document(
                                &ProductListingPercolationInput {
                                    source: product,
                                    valuation: Some(ProductListingPercolationValuation {
                                        basis: ProductListingPriceValuationBasis::Event,
                                        fx_rate_id: snapshot.id(),
                                        effective_at: snapshot.captured_at(),
                                        prices,
                                    }),
                                },
                            )?;
                        let normal_membership = normal_search_membership(
                            &normal_price_clause,
                            source_currency,
                            source_amount,
                        )?;
                        let saved_filter_membership = saved_filter_percolation_membership(
                            &product_listing_percolator_query,
                            &product_listing_percolation_document,
                        )?;

                        assert_eq!(
                            normal_membership, saved_filter_membership,
                            "{source_currency:?} -> {target_currency:?}, source amount {source_amount}, range {range:?}",
                        );
                    }
                }
            }
        }

        let supported_currency_count = Currency::iter().count();
        assert_eq!(
            supported_currency_count * supported_currency_count,
            covered_pairs
        );
        assert_eq!(
            supported_currency_count * 2 - 1,
            covered_jpy_pairs,
            "all JPY source and target pairs must be covered"
        );
        Ok(())
    }

    #[test]
    fn should_serialize_every_supported_currency_in_closed_world_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(ProductListingPercolationPricesDocument {
            eur: 1,
            gbp: 1,
            usd: 1,
            aud: 1,
            cad: 1,
            nzd: 1,
            cny: 1,
            brl: 1,
            pln: 1,
            r#try: 1,
            jpy: 1,
            czk: 1,
            rub: 1,
            aed: 1,
            sar: 1,
            hkd: 1,
            sgd: 1,
            chf: 1,
        })?;

        assert_eq!(18, value.as_object().map_or(0, serde_json::Map::len));
        assert!(value.get("jpy").is_some());
        assert!(value.get("priceEstimateMin").is_none());
        Ok(())
    }
}
