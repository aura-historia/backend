use crate::{
    continent_document::ContinentDocument, product_document::ProductDocument,
    product_image_document::ProductImageDocument, product_state_document::ProductStateDocument,
    shop_type_document::ShopTypeDocument,
};
use common::{
    currency::domain::Currency,
    language::{
        document::{LanguageDocument, TextDocument},
        domain::Language,
    },
    price::domain::Price,
    product_lifecycle::document::ProductLifecycleDocument,
};
use product_service::ports::{ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource};
use serde_json::Value;

/// Builds the canonical Product JSON consumed by search-filter percolation.
///
/// This is intentionally JSON-only. `ProductDocument` stays private to this adapter.
#[derive(Debug, thiserror::Error)]
pub enum ProductPercolationDocumentError {
    #[error("product percolation document serialization failed")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

pub fn product_percolation_document(
    product: &ProductSearchFilterMatchSource,
) -> Result<Value, ProductPercolationDocumentError> {
    serde_json::to_value(ProductDocument::from(product))
        .map_err(|source| ProductPercolationDocumentError::Serialize { source })
}

impl From<&ProductSearchFilterMatchSource> for ProductDocument {
    fn from(product: &ProductSearchFilterMatchSource) -> Self {
        let (title, language) = selected_title(product);
        let structured_address = product.address.structured.as_ref();

        Self {
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
            structured_address_region: structured_address
                .and_then(|address| address.region.clone()),
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
            price_eur: price_amount_in(product.pricing.price, Currency::Eur),
            price_usd: price_amount_in(product.pricing.price, Currency::Usd),
            price_gbp: price_amount_in(product.pricing.price, Currency::Gbp),
            price_aud: price_amount_in(product.pricing.price, Currency::Aud),
            price_cad: price_amount_in(product.pricing.price, Currency::Cad),
            price_nzd: price_amount_in(product.pricing.price, Currency::Nzd),
            price_cny: price_amount_in(product.pricing.price, Currency::Cny),
            price_brl: price_amount_in(product.pricing.price, Currency::Brl),
            price_pln: price_amount_in(product.pricing.price, Currency::Pln),
            price_try: price_amount_in(product.pricing.price, Currency::Try),
            price_jpy: price_amount_in(product.pricing.price, Currency::Jpy),
            price_czk: price_amount_in(product.pricing.price, Currency::Czk),
            price_rub: price_amount_in(product.pricing.price, Currency::Rub),
            price_aed: price_amount_in(product.pricing.price, Currency::Aed),
            price_sar: price_amount_in(product.pricing.price, Currency::Sar),
            price_hkd: price_amount_in(product.pricing.price, Currency::Hkd),
            price_sgd: price_amount_in(product.pricing.price, Currency::Sgd),
            price_chf: price_amount_in(product.pricing.price, Currency::Chf),
            price_estimate_min_eur: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Eur,
            ),
            price_estimate_min_usd: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Usd,
            ),
            price_estimate_min_gbp: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Gbp,
            ),
            price_estimate_min_aud: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Aud,
            ),
            price_estimate_min_cad: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Cad,
            ),
            price_estimate_min_nzd: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Nzd,
            ),
            price_estimate_min_cny: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Cny,
            ),
            price_estimate_min_brl: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Brl,
            ),
            price_estimate_min_pln: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Pln,
            ),
            price_estimate_min_try: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Try,
            ),
            price_estimate_min_jpy: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Jpy,
            ),
            price_estimate_min_czk: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Czk,
            ),
            price_estimate_min_rub: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Rub,
            ),
            price_estimate_min_aed: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Aed,
            ),
            price_estimate_min_sar: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Sar,
            ),
            price_estimate_min_hkd: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Hkd,
            ),
            price_estimate_min_sgd: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Sgd,
            ),
            price_estimate_min_chf: price_amount_in(
                product.pricing.price_estimate_min,
                Currency::Chf,
            ),
            price_estimate_max_eur: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Eur,
            ),
            price_estimate_max_usd: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Usd,
            ),
            price_estimate_max_gbp: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Gbp,
            ),
            price_estimate_max_aud: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Aud,
            ),
            price_estimate_max_cad: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Cad,
            ),
            price_estimate_max_nzd: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Nzd,
            ),
            price_estimate_max_cny: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Cny,
            ),
            price_estimate_max_brl: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Brl,
            ),
            price_estimate_max_pln: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Pln,
            ),
            price_estimate_max_try: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Try,
            ),
            price_estimate_max_jpy: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Jpy,
            ),
            price_estimate_max_czk: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Czk,
            ),
            price_estimate_max_rub: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Rub,
            ),
            price_estimate_max_aed: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Aed,
            ),
            price_estimate_max_sar: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Sar,
            ),
            price_estimate_max_hkd: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Hkd,
            ),
            price_estimate_max_sgd: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Sgd,
            ),
            price_estimate_max_chf: price_amount_in(
                product.pricing.price_estimate_max,
                Currency::Chf,
            ),
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
            embedding: None,
            auction_start: product.auction.start,
            auction_end: product.auction.end,
            created: product.created,
            updated: product.updated,
        }
    }
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

fn price_amount_in(price: Option<Price>, currency: Currency) -> Option<u64> {
    price
        .filter(|price| price.currency == currency)
        .map(|price| u64::from(price.monetary_amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
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
    use indexmap::IndexSet;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        title::Title,
    };
    use product_service::ports::ProductSearchFilterMatchSourceEventKind;
    use serde_json::json;
    use std::collections::HashMap;
    use url::Url;

    fn source() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let title = Title::from("Blue vase");
        let url = Url::parse("https://shop.example.test/products/blue-vase")?;
        let event_id = EventId::new();
        Ok(ProductSearchFilterMatchSource {
            event_id,
            event_kind: ProductSearchFilterMatchSourceEventKind::Domain,
            current_event_id: event_id,
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
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            auction: ProductAuction::default(),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    #[test]
    fn should_map_typed_product_source_to_canonical_percolation_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source()?;

        let document = product_percolation_document(&source)?;

        assert_eq!(document["productId"], json!(source.product_id));
        assert_eq!(document["title"]["text"], json!("Blue vase"));
        assert_eq!(document["title"]["language"], json!("EN"));
        assert_eq!(document["titleEn"], json!("Blue vase"));
        assert_eq!(document["priceEur"], json!(125));
        assert_eq!(document["shopType"], json!("MARKETPLACE"));
        assert_eq!(document["state"], json!("AVAILABLE"));
        assert_eq!(document["lifecycle"], json!("ACTIVE"));
        Ok(())
    }
}
