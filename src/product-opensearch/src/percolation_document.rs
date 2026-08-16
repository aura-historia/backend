use crate::{
    continent_document::ContinentDocument,
    product_document::{ProductDocument, SourcePriceDocument},
    product_image_document::ProductImageDocument,
    product_state_document::ProductStateDocument,
    shop_type_document::ShopTypeDocument,
};
use common::{
    currency::{data::CurrencyData, domain::Currency},
    fx_rate_id::FxRateId,
    language::{
        document::{LanguageDocument, TextDocument},
        domain::Language,
    },
    product_lifecycle::document::ProductLifecycleDocument,
};
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use product_service::ports::{ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource};
use serde_json::Value;

/// Builds canonical Product JSON consumed by search-filter percolation.
///
/// Source pricing stays native. Sale pricing is rendered only from the exact
/// immutable sale snapshot; percolation never invents converted values.
#[derive(Debug, thiserror::Error)]
pub enum ProductPercolationDocumentError {
    #[error("sold product source is missing immutable sale valuation")]
    MissingSaleValuation,
    #[error("sale valuation is missing its immutable FX snapshot")]
    MissingSaleSnapshot,
    #[error("active product source must not receive a sale FX snapshot")]
    UnexpectedSaleSnapshot,
    #[error("sale valuation FX snapshot ID does not match the supplied snapshot")]
    SaleSnapshotMismatch {
        valuation_fx_rate_id: FxRateId,
        snapshot_fx_rate_id: FxRateId,
    },
    #[error("sale valuation cannot be projected without a native source price")]
    MissingSalePrice,
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

pub fn product_percolation_document(
    product: &ProductSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<Value, ProductPercolationDocumentError> {
    serde_json::to_value(product_document(product, sale_snapshot)?)
        .map_err(|source| ProductPercolationDocumentError::Serialize { source })
}

pub(crate) fn product_document(
    product: &ProductSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<ProductDocument, ProductPercolationDocumentError> {
    let (sale_prices, sale_fx_rate_id, sold_at) = sale_projection(product, sale_snapshot)?;
    let (title, language) = selected_title(product);
    let structured_address = product.address.structured.as_ref();

    Ok(ProductDocument {
        product_id: product.product_id,
        product_slug_id: product.product_slug_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        seller_slug_id: product.seller_slug_id.clone(),
        event_id: product.current_event_id,
        shop_id: product.shop_id,
        seller_id: product.seller_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_name: product.shop_name.to_string(),
        seller_name: product.seller_name.to_string(),
        shop_type: product.shop_type.into(),
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
        structured_address_continent: structured_address
            .and_then(|address| address.continent)
            .map(ContinentDocument::from),
        geo_address: product
            .address
            .geo
            .as_ref()
            .map(|geo| format!("{},{}", geo.lat, geo.lon)),
        title: TextDocument::new(title, LanguageDocument::from(language)),
        title_de: translated_title(product, Language::De),
        title_en: translated_title(product, Language::En),
        title_fr: translated_title(product, Language::Fr),
        title_es: translated_title(product, Language::Es),
        title_it: translated_title(product, Language::It),
        source_price: product.pricing.price.map(|price| SourcePriceDocument {
            amount: price.monetary_amount.into(),
            currency: CurrencyData::from(price.currency),
        }),
        sale_prices,
        sale_fx_rate_id,
        sold_at,
        state: ProductStateDocument::from(product.state),
        lifecycle: ProductLifecycleDocument::from(product.lifecycle),
        url: product.url.clone(),
        view_url: product.view_url.clone(),
        images: product
            .images
            .iter()
            .cloned()
            .map(ProductImageDocument::from)
            .collect(),
        embedding: product.embedding.clone(),
        auction_start: product.auction.start,
        auction_end: product.auction.end,
        created: product.created,
        updated: product.updated,
    })
}

type SaleProjection = (
    Option<crate::product_document::SalePricesDocument>,
    Option<FxRateId>,
    Option<time::OffsetDateTime>,
);

fn sale_projection(
    product: &ProductSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<SaleProjection, ProductPercolationDocumentError> {
    match (product.sale_valuation, sale_snapshot) {
        (None, None) if product.state != common::product_state::domain::ProductState::Sold => {
            Ok((None, None, None))
        }
        (None, None) => Err(ProductPercolationDocumentError::MissingSaleValuation),
        (None, Some(_)) => Err(ProductPercolationDocumentError::UnexpectedSaleSnapshot),
        (Some(_), None) => Err(ProductPercolationDocumentError::MissingSaleSnapshot),
        (Some(valuation), Some(snapshot)) if valuation.fx_rate_id != snapshot.id() => {
            Err(ProductPercolationDocumentError::SaleSnapshotMismatch {
                valuation_fx_rate_id: valuation.fx_rate_id,
                snapshot_fx_rate_id: snapshot.id(),
            })
        }
        (Some(valuation), Some(snapshot)) => Ok((
            Some(sale_prices(
                snapshot,
                product
                    .pricing
                    .price
                    .ok_or(ProductPercolationDocumentError::MissingSalePrice)?,
            )?),
            Some(valuation.fx_rate_id),
            Some(valuation.sold_at),
        )),
    }
}

fn sale_prices(
    snapshot: &FxRateSnapshot,
    source_price: common::price::domain::Price,
) -> Result<crate::product_document::SalePricesDocument, ProductPercolationDocumentError> {
    let amount_in = |currency| {
        snapshot
            .convert(source_price, currency, RoundingMode::HalfUp)
            .map(|price| u64::from(price.monetary_amount))
            .map_err(|source| ProductPercolationDocumentError::InvalidSaleSnapshot { source })
    };

    Ok(crate::product_document::SalePricesDocument {
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

impl From<ProductSearchFilterMatchShopType> for ShopTypeDocument {
    fn from(value: ProductSearchFilterMatchShopType) -> Self {
        match value {
            ProductSearchFilterMatchShopType::AuctionHouse => Self::AuctionHouse,
            ProductSearchFilterMatchShopType::AuctionPlatform => Self::AuctionPlatform,
            ProductSearchFilterMatchShopType::CommercialDealer => Self::CommercialDealer,
            ProductSearchFilterMatchShopType::Marketplace => Self::Marketplace,
        }
    }
}

fn selected_title(product: &ProductSearchFilterMatchSource) -> (&str, Language) {
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
    product: &ProductSearchFilterMatchSource,
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
    use common::{
        currency::domain::Currency,
        event_id::EventId,
        localized::Localized,
        price::domain::{MonetaryAmount, Price},
        product_lifecycle::domain::ProductLifecycle,
        product_slug_id::ProductSlugId,
        product_state::domain::ProductState,
        shop_id::ShopId,
        shop_name::ShopName,
        shop_slug_id::ShopSlugId,
        shops_product_id::ShopsProductId,
    };
    use fxrate_core::{
        FX_RATE_SCALE, FxRateQuote, FxRateSnapshot, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation},
        title::Title,
    };
    use product_service::ports::ProductSearchFilterMatchSourceEventKind;
    use serde_json::json;
    use std::collections::HashMap;
    use strum::IntoEnumIterator;
    use url::Url;

    fn source() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let title = Title::from("Blue vase");
        let url = Url::parse("https://shop.example.test/products/blue-vase")?;
        let event_id = EventId::new();
        Ok(ProductSearchFilterMatchSource {
            event_id,
            event_kind: ProductSearchFilterMatchSourceEventKind::Domain,
            current_event_id: event_id,
            projection_version: 1,
            product_id: common::product_id::ProductId::new(),
            product_slug_id: ProductSlugId::from("blue-vase"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: common::seller_slug_id::SellerSlugId::from("seller"),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("sku-1"),
            address: ProductAddress::default(),
            product_title: Some(Localized::new(Language::En, title.clone())),
            product_description: None,
            titles: HashMap::from([(Language::En, title)]),
            descriptions: HashMap::new(),
            pricing: ProductPricing {
                price: Some(Price::new(MonetaryAmount::from(125_u64), Currency::Eur)),
                ..Default::default()
            },
            sale_valuation: None,
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: ProductAuction::default(),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn snapshot(fx_rate_id: FxRateId) -> Result<FxRateSnapshot, fxrate_core::FxRateSnapshotError> {
        snapshot_with_usd_quote(fx_rate_id, 1_250_000)
    }

    fn snapshot_with_usd_quote(
        fx_rate_id: FxRateId,
        usd_quote: u64,
    ) -> Result<FxRateSnapshot, fxrate_core::FxRateSnapshotError> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            fx_rate_id,
            time::OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Eur => FX_RATE_SCALE,
                        Currency::Usd => usd_quote,
                        _ => FX_RATE_SCALE,
                    },
                )
            }),
        )?;
        Ok(snapshot.into_persisted(1_i64.try_into()?))
    }

    #[test]
    fn should_map_native_source_price_without_converted_sale_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source()?;

        let document = product_percolation_document(&source, None)?;

        assert_eq!(
            document["sourcePrice"],
            json!({ "amount": 125, "currency": "EUR" })
        );
        assert!(document.get("salePrices").is_none());
        assert!(document.get("priceEur").is_none());
        assert!(document.get("priceEstimateMinEur").is_none());
        Ok(())
    }

    #[test]
    fn should_render_all_sale_prices_from_the_immutable_sale_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source()?;
        let fx_rate_id = FxRateId::new();
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id,
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;
        let sale_snapshot = snapshot(fx_rate_id)?;

        let document = product_percolation_document(&source, Some(&sale_snapshot))?;

        for currency in Currency::iter() {
            assert!(
                document["salePrices"]
                    .get(currency.as_str().to_lowercase())
                    .is_some(),
                "missing sale price for {currency}"
            );
        }
        assert_eq!(json!(156), document["salePrices"]["usd"]);
        assert_eq!(json!(fx_rate_id), document["saleFxRateId"]);
        assert_eq!(json!("1970-01-01T00:00:00Z"), document["soldAt"]);
        Ok(())
    }

    #[test]
    fn should_round_sale_prices_half_up_from_immutable_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(101_u64), Currency::Eur));
        let fx_rate_id = FxRateId::new();
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id,
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;
        let sale_snapshot = snapshot_with_usd_quote(fx_rate_id, 1_500_000)?;

        let document = product_percolation_document(&source, Some(&sale_snapshot))?;

        assert_eq!(json!(152), document["salePrices"]["usd"]);
        Ok(())
    }

    #[test]
    fn should_reject_missing_sale_source_price() -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source()?;
        let fx_rate_id = FxRateId::new();
        source.pricing.price = None;
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id,
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;
        let sale_snapshot = snapshot(fx_rate_id)?;

        assert!(matches!(
            product_percolation_document(&source, Some(&sale_snapshot)),
            Err(ProductPercolationDocumentError::MissingSalePrice)
        ));
        Ok(())
    }

    #[test]
    fn should_reject_missing_sale_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source()?;
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: FxRateId::new(),
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;

        assert!(matches!(
            product_percolation_document(&source, None),
            Err(ProductPercolationDocumentError::MissingSaleSnapshot)
        ));
        Ok(())
    }

    #[test]
    fn should_reject_mismatched_sale_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source()?;
        let valuation_fx_rate_id = FxRateId::new();
        let snapshot_fx_rate_id = FxRateId::new();
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: valuation_fx_rate_id,
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;
        let sale_snapshot = snapshot(snapshot_fx_rate_id)?;

        assert!(matches!(
            product_percolation_document(&source, Some(&sale_snapshot)),
            Err(ProductPercolationDocumentError::SaleSnapshotMismatch {
                valuation_fx_rate_id: actual_valuation,
                snapshot_fx_rate_id: actual_snapshot,
            }) if actual_valuation == valuation_fx_rate_id && actual_snapshot == snapshot_fx_rate_id
        ));
        Ok(())
    }
}
