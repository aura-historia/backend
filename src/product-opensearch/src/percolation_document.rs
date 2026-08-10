use crate::product_document::ProductDocument;
use common::{
    currency::domain::Currency,
    language::{document::LanguageDocument, domain::Language},
    price::domain::Price,
    product_lifecycle::domain::ProductLifecycle,
    product_state::domain::ProductState,
};
use product_service::ports::{ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource};
use serde_json::{Map, Value, json};
use time::format_description::well_known::Rfc3339;

/// Builds the canonical Product JSON consumed by search-filter percolation.
///
/// This is intentionally JSON-only. `ProductDocument` stays private to this adapter.
#[derive(Debug, thiserror::Error)]
pub enum ProductPercolationDocumentError {
    #[error("product percolation timestamp formatting failed")]
    Timestamp(#[source] time::error::Format),
    #[error("product percolation country serialization failed")]
    Country(#[source] serde_json::Error),
    #[error("product percolation document is invalid")]
    InvalidDocument {
        #[source]
        source: serde_json::Error,
    },
    #[error("product percolation document serialization failed")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

pub fn product_percolation_document(
    product: &ProductSearchFilterMatchSource,
) -> Result<Value, ProductPercolationDocumentError> {
    // Decode through the private canonical Product document before serializing. This keeps the
    // percolation payload aligned with the product index without exporting its storage type.
    let document = serde_json::from_value::<ProductDocument>(percolation_fields(product)?)
        .map_err(|source| ProductPercolationDocumentError::InvalidDocument { source })?;
    serde_json::to_value(document)
        .map_err(|source| ProductPercolationDocumentError::Serialize { source })
}

fn percolation_fields(
    product: &ProductSearchFilterMatchSource,
) -> Result<Value, ProductPercolationDocumentError> {
    let mut document = Map::new();
    document.insert("productId".to_owned(), json!(product.product_id));
    document.insert("productSlugId".to_owned(), json!(product.product_slug_id));
    document.insert("shopId".to_owned(), json!(product.shop_id));
    document.insert("shopSlugId".to_owned(), json!(product.shop_slug_id));
    document.insert("shopName".to_owned(), json!(product.shop_name));
    document.insert("shopType".to_owned(), json!(shop_type(product.shop_type)));
    document.insert("sellerId".to_owned(), json!(product.seller_id));
    document.insert("sellerSlugId".to_owned(), json!(product.seller_slug_id));
    document.insert("sellerName".to_owned(), json!(product.seller_name));
    document.insert("shopsProductId".to_owned(), json!(product.shops_product_id));
    document.insert("eventId".to_owned(), json!(product.current_event_id));
    document.insert("state".to_owned(), json!(product_state(product.state)));
    document.insert("lifecycle".to_owned(), json!(lifecycle(product.lifecycle)));
    document.insert("url".to_owned(), json!(product.url.as_str()));
    document.insert("viewUrl".to_owned(), json!(product.view_url.as_str()));
    document.insert(
        "created".to_owned(),
        json!(
            product
                .created
                .format(&Rfc3339)
                .map_err(ProductPercolationDocumentError::Timestamp)?
        ),
    );
    document.insert(
        "updated".to_owned(),
        json!(
            product
                .updated
                .format(&Rfc3339)
                .map_err(ProductPercolationDocumentError::Timestamp)?
        ),
    );
    if let Some(auction_start) = product.auction.start {
        document.insert(
            "auctionStart".to_owned(),
            json!(
                auction_start
                    .format(&Rfc3339)
                    .map_err(ProductPercolationDocumentError::Timestamp)?
            ),
        );
    }
    if let Some(auction_end) = product.auction.end {
        document.insert(
            "auctionEnd".to_owned(),
            json!(
                auction_end
                    .format(&Rfc3339)
                    .map_err(ProductPercolationDocumentError::Timestamp)?
            ),
        );
    }

    let (title, language) = selected_title(product);
    document.insert(
        "title".to_owned(),
        json!({"text": title, "language": LanguageDocument::from(language)}),
    );
    for (language, field) in [
        (Language::De, "titleDe"),
        (Language::En, "titleEn"),
        (Language::Fr, "titleFr"),
        (Language::Es, "titleEs"),
        (Language::It, "titleIt"),
    ] {
        if let Some(title) = product.titles.get(&language) {
            document.insert(field.to_owned(), json!(title.as_ref()));
        }
    }

    insert_price(&mut document, "price", product.pricing.price);
    insert_price(
        &mut document,
        "priceEstimateMin",
        product.pricing.price_estimate_min,
    );
    insert_price(
        &mut document,
        "priceEstimateMax",
        product.pricing.price_estimate_max,
    );

    if let Some(structured) = &product.address.structured {
        insert_optional_string(
            &mut document,
            "structuredAddressAddressline",
            structured.addressline.as_deref(),
        );
        insert_optional_string(
            &mut document,
            "structuredAddressAddresslineExtra",
            structured.addressline_extra.as_deref(),
        );
        insert_optional_string(
            &mut document,
            "structuredAddressLocality",
            structured.locality.as_deref(),
        );
        insert_optional_string(
            &mut document,
            "structuredAddressRegion",
            structured.region.as_deref(),
        );
        insert_optional_string(
            &mut document,
            "structuredAddressPostalCode",
            structured.postal_code.as_deref(),
        );
        if let Some(country) = structured.country {
            document.insert(
                "structuredAddressCountry".to_owned(),
                serde_json::to_value(country).map_err(ProductPercolationDocumentError::Country)?,
            );
        }
        if let Some(continent) = structured.continent {
            document.insert(
                "structuredAddressContinent".to_owned(),
                json!(continent_name(continent)),
            );
        }
    }
    if let Some(geo) = product.address.geo {
        document.insert(
            "geoAddress".to_owned(),
            json!(format!("{},{}", geo.lat, geo.lon)),
        );
    }

    Ok(Value::Object(document))
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

fn insert_optional_string(document: &mut Map<String, Value>, field: &str, value: Option<&str>) {
    if let Some(value) = value {
        document.insert(field.to_owned(), json!(value));
    }
}

fn insert_price(document: &mut Map<String, Value>, prefix: &str, price: Option<Price>) {
    let Some(price) = price else {
        return;
    };
    document.insert(
        format!("{prefix}{}", currency_suffix(price.currency)),
        json!(u64::from(price.monetary_amount)),
    );
}

fn currency_suffix(currency: Currency) -> &'static str {
    match currency {
        Currency::Eur => "Eur",
        Currency::Gbp => "Gbp",
        Currency::Usd => "Usd",
        Currency::Aud => "Aud",
        Currency::Cad => "Cad",
        Currency::Nzd => "Nzd",
        Currency::Cny => "Cny",
        Currency::Brl => "Brl",
        Currency::Pln => "Pln",
        Currency::Try => "Try",
        Currency::Jpy => "Jpy",
        Currency::Czk => "Czk",
        Currency::Rub => "Rub",
        Currency::Aed => "Aed",
        Currency::Sar => "Sar",
        Currency::Hkd => "Hkd",
        Currency::Sgd => "Sgd",
        Currency::Chf => "Chf",
    }
}

fn shop_type(value: ProductSearchFilterMatchShopType) -> &'static str {
    match value {
        ProductSearchFilterMatchShopType::AuctionHouse => "AUCTION_HOUSE",
        ProductSearchFilterMatchShopType::AuctionPlatform => "AUCTION_PLATFORM",
        ProductSearchFilterMatchShopType::CommercialDealer => "COMMERCIAL_DEALER",
        ProductSearchFilterMatchShopType::Marketplace => "MARKETPLACE",
    }
}

fn product_state(value: ProductState) -> &'static str {
    match value {
        ProductState::Listed => "LISTED",
        ProductState::Available => "AVAILABLE",
        ProductState::Reserved => "RESERVED",
        ProductState::Sold => "SOLD",
        ProductState::Removed => "REMOVED",
        ProductState::Unknown => "UNKNOWN",
    }
}

fn lifecycle(value: ProductLifecycle) -> &'static str {
    match value {
        ProductLifecycle::Active => "ACTIVE",
        ProductLifecycle::Deleted => "DELETED",
    }
}

fn continent_name(value: geo::core::continent::Continent) -> &'static str {
    match value {
        geo::core::continent::Continent::Africa => "AFRICA",
        geo::core::continent::Continent::Antarctica => "ANTARCTICA",
        geo::core::continent::Continent::Asia => "ASIA",
        geo::core::continent::Continent::Europe => "EUROPE",
        geo::core::continent::Continent::NorthAmerica => "NORTH_AMERICA",
        geo::core::continent::Continent::Oceania => "OCEANIA",
        geo::core::continent::Continent::SouthAmerica => "SOUTH_AMERICA",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        event_id::EventId,
        localized::Localized,
        price::domain::{MonetaryAmount, Price},
        product_slug_id::ProductSlugId,
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
    use std::collections::HashMap;
    use url::Url;

    fn source() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let title = Title::from("Blue vase");
        let url = Url::parse("https://shop.example.test/products/blue-vase")?;
        let event_id = EventId::new();
        Ok(ProductSearchFilterMatchSource {
            event_id,
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
