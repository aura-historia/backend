use crate::product_image_document::ProductImageDocument;
use crate::product_state_document::ProductStateDocument;
use crate::shop_type_document::ShopTypeDocument;
use common::event_id::EventId;
use common::language::document::TextDocument;
use common::product_id::ProductId;
use common::product_lifecycle::document::ProductLifecycleDocument;
use common::product_slug_id::ProductSlugId;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use indexmap::IndexSet;
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductDocument {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: SellerSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub seller_name: String,
    pub shop_type: ShopTypeDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline_extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_country: Option<CountryCode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_continent: Option<crate::continent_document::ContinentDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<String>,
    pub title: TextDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_chf: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_chf: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_chf: Option<u64>,
    pub state: ProductStateDocument,
    pub lifecycle: ProductLifecycleDocument,
    pub url: Url,
    pub view_url: Url,
    #[serde(skip_serializing_if = "IndexSet::is_empty", default)]
    pub images: IndexSet<ProductImageDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_end: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ProductDocument {
    pub(crate) fn _id(&self) -> ProductId {
        self.product_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_state_document::ProductStateDocument;
    use crate::prohibited_content_document::ProhibitedContentDocument;
    use crate::shop_type_document::ShopTypeDocument;
    use common::language::document::LanguageDocument;
    use time::macros::datetime;

    fn document() -> Result<ProductDocument, url::ParseError> {
        Ok(ProductDocument {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from("vase-abcdef"),
            shop_slug_id: ShopSlugId::from("shop"),
            seller_slug_id: SellerSlugId::from("seller"),
            event_id: EventId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("sku-1"),
            shop_name: "Shop".to_owned(),
            seller_name: "Seller".to_owned(),
            shop_type: ShopTypeDocument::CommercialDealer,
            structured_address_addressline: Some("Street 1".to_owned()),
            structured_address_addressline_extra: None,
            structured_address_locality: Some("Berlin".to_owned()),
            structured_address_region: None,
            structured_address_postal_code: Some("10115".to_owned()),
            structured_address_country: Some(CountryCode::DEU),
            structured_address_continent: Some(
                crate::continent_document::ContinentDocument::Europe,
            ),
            geo_address: Some("52.52,13.405".to_owned()),
            title: TextDocument::new("Vase", LanguageDocument::En),
            title_de: Some("Vase".to_owned()),
            title_en: Some("Vase".to_owned()),
            title_fr: None,
            title_es: None,
            title_it: None,
            price_eur: Some(100),
            price_usd: Some(110),
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            price_cny: None,
            price_brl: None,
            price_pln: None,
            price_try: None,
            price_jpy: None,
            price_czk: None,
            price_rub: None,
            price_aed: None,
            price_sar: None,
            price_hkd: None,
            price_sgd: None,
            price_chf: None,
            price_estimate_min_eur: None,
            price_estimate_min_usd: None,
            price_estimate_min_gbp: None,
            price_estimate_min_aud: None,
            price_estimate_min_cad: None,
            price_estimate_min_nzd: None,
            price_estimate_min_cny: None,
            price_estimate_min_brl: None,
            price_estimate_min_pln: None,
            price_estimate_min_try: None,
            price_estimate_min_jpy: None,
            price_estimate_min_czk: None,
            price_estimate_min_rub: None,
            price_estimate_min_aed: None,
            price_estimate_min_sar: None,
            price_estimate_min_hkd: None,
            price_estimate_min_sgd: None,
            price_estimate_min_chf: None,
            price_estimate_max_eur: None,
            price_estimate_max_usd: None,
            price_estimate_max_gbp: None,
            price_estimate_max_aud: None,
            price_estimate_max_cad: None,
            price_estimate_max_nzd: None,
            price_estimate_max_cny: None,
            price_estimate_max_brl: None,
            price_estimate_max_pln: None,
            price_estimate_max_try: None,
            price_estimate_max_jpy: None,
            price_estimate_max_czk: None,
            price_estimate_max_rub: None,
            price_estimate_max_aed: None,
            price_estimate_max_sar: None,
            price_estimate_max_hkd: None,
            price_estimate_max_sgd: None,
            price_estimate_max_chf: None,
            state: ProductStateDocument::Available,
            lifecycle: ProductLifecycleDocument::Active,
            url: Url::parse("https://shop.example/products/sku-1")?,
            view_url: Url::parse("https://aura.example/products/vase-abcdef")?,
            images: [ProductImageDocument {
                url: Url::parse("https://example.com/image.jpg")?,
                prohibited_content: ProhibitedContentDocument::None,
            }]
            .into_iter()
            .collect(),
            embedding: Some(vec![0.1, 0.2]),
            auction_start: None,
            auction_end: Some(datetime!(2026-01-01 0:00 UTC)),
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        })
    }

    #[test]
    fn should_serialize_without_actor_fields() -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(document()?)?;

        assert!(value.get("createdBy").is_none());
        assert!(value.get("updatedBy").is_none());
        assert!(value.get("created").is_some());
        assert!(value.get("updated").is_some());
        Ok(())
    }

    #[test]
    fn should_roundtrip_product_document() -> Result<(), Box<dyn std::error::Error>> {
        let expected = document()?;

        let actual = serde_json::from_value::<ProductDocument>(serde_json::to_value(&expected)?)?;

        assert_eq!(expected, actual);
        Ok(())
    }

    #[test]
    fn should_return_product_document_id() -> Result<(), url::ParseError> {
        let document = document()?;

        assert_eq!(document.product_id, document._id());
        Ok(())
    }
}
