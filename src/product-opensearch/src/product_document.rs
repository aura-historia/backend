use crate::product_image_document::ProductImageDocument;
use crate::product_state_document::ProductStateDocument;
use crate::shop_type_document::ShopTypeDocument;
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::product_id::ProductId;
use common::product_lifecycle::document::ProductLifecycleDocument;
use common::product_slug_id::ProductSlugId;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use indexmap::IndexSet;
use isocountry::CountryCode;
use localization::Language;
use money::Currency;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum CurrencyDocument {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
    Cny,
    Brl,
    Pln,
    Try,
    Jpy,
    Czk,
    Rub,
    Aed,
    Sar,
    Hkd,
    Sgd,
    Chf,
}

impl From<Currency> for CurrencyDocument {
    fn from(currency: Currency) -> Self {
        match currency {
            Currency::Eur => Self::Eur,
            Currency::Gbp => Self::Gbp,
            Currency::Usd => Self::Usd,
            Currency::Aud => Self::Aud,
            Currency::Cad => Self::Cad,
            Currency::Nzd => Self::Nzd,
            Currency::Cny => Self::Cny,
            Currency::Brl => Self::Brl,
            Currency::Pln => Self::Pln,
            Currency::Try => Self::Try,
            Currency::Jpy => Self::Jpy,
            Currency::Czk => Self::Czk,
            Currency::Rub => Self::Rub,
            Currency::Aed => Self::Aed,
            Currency::Sar => Self::Sar,
            Currency::Hkd => Self::Hkd,
            Currency::Sgd => Self::Sgd,
            Currency::Chf => Self::Chf,
        }
    }
}

impl From<CurrencyDocument> for Currency {
    fn from(currency: CurrencyDocument) -> Self {
        match currency {
            CurrencyDocument::Eur => Self::Eur,
            CurrencyDocument::Gbp => Self::Gbp,
            CurrencyDocument::Usd => Self::Usd,
            CurrencyDocument::Aud => Self::Aud,
            CurrencyDocument::Cad => Self::Cad,
            CurrencyDocument::Nzd => Self::Nzd,
            CurrencyDocument::Cny => Self::Cny,
            CurrencyDocument::Brl => Self::Brl,
            CurrencyDocument::Pln => Self::Pln,
            CurrencyDocument::Try => Self::Try,
            CurrencyDocument::Jpy => Self::Jpy,
            CurrencyDocument::Czk => Self::Czk,
            CurrencyDocument::Rub => Self::Rub,
            CurrencyDocument::Aed => Self::Aed,
            CurrencyDocument::Sar => Self::Sar,
            CurrencyDocument::Hkd => Self::Hkd,
            CurrencyDocument::Sgd => Self::Sgd,
            CurrencyDocument::Chf => Self::Chf,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum LanguageDocument {
    De,
    En,
    Fr,
    Es,
    It,
    Zh,
    Pt,
    Pl,
    Tr,
    Nl,
    Cs,
    Ja,
    Ru,
    Ar,
}

impl From<Language> for LanguageDocument {
    fn from(language: Language) -> Self {
        match language {
            Language::De => Self::De,
            Language::En => Self::En,
            Language::Fr => Self::Fr,
            Language::Es => Self::Es,
            Language::It => Self::It,
            Language::Zh => Self::Zh,
            Language::Pt => Self::Pt,
            Language::Pl => Self::Pl,
            Language::Tr => Self::Tr,
            Language::Nl => Self::Nl,
            Language::Cs => Self::Cs,
            Language::Ja => Self::Ja,
            Language::Ru => Self::Ru,
            Language::Ar => Self::Ar,
        }
    }
}

impl From<LanguageDocument> for Language {
    fn from(language: LanguageDocument) -> Self {
        match language {
            LanguageDocument::De => Self::De,
            LanguageDocument::En => Self::En,
            LanguageDocument::Fr => Self::Fr,
            LanguageDocument::Es => Self::Es,
            LanguageDocument::It => Self::It,
            LanguageDocument::Zh => Self::Zh,
            LanguageDocument::Pt => Self::Pt,
            LanguageDocument::Pl => Self::Pl,
            LanguageDocument::Tr => Self::Tr,
            LanguageDocument::Nl => Self::Nl,
            LanguageDocument::Cs => Self::Cs,
            LanguageDocument::Ja => Self::Ja,
            LanguageDocument::Ru => Self::Ru,
            LanguageDocument::Ar => Self::Ar,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TextDocument {
    pub(crate) text: String,
    pub(crate) language: LanguageDocument,
}

impl TextDocument {
    pub(crate) fn new(text: impl Into<String>, language: LanguageDocument) -> Self {
        Self {
            text: text.into(),
            language,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourcePriceDocument {
    pub(crate) amount: u64,
    pub(crate) currency: CurrencyDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SalePricesDocument {
    pub(crate) eur: u64,
    pub(crate) gbp: u64,
    pub(crate) usd: u64,
    pub(crate) aud: u64,
    pub(crate) cad: u64,
    pub(crate) nzd: u64,
    pub(crate) cny: u64,
    pub(crate) brl: u64,
    pub(crate) pln: u64,
    pub(crate) r#try: u64,
    pub(crate) jpy: u64,
    pub(crate) czk: u64,
    pub(crate) rub: u64,
    pub(crate) aed: u64,
    pub(crate) sar: u64,
    pub(crate) hkd: u64,
    pub(crate) sgd: u64,
    pub(crate) chf: u64,
}

impl SalePricesDocument {
    fn amount_in(&self, currency: Currency) -> u64 {
        match currency {
            Currency::Eur => self.eur,
            Currency::Gbp => self.gbp,
            Currency::Usd => self.usd,
            Currency::Aud => self.aud,
            Currency::Cad => self.cad,
            Currency::Nzd => self.nzd,
            Currency::Cny => self.cny,
            Currency::Brl => self.brl,
            Currency::Pln => self.pln,
            Currency::Try => self.r#try,
            Currency::Jpy => self.jpy,
            Currency::Czk => self.czk,
            Currency::Rub => self.rub,
            Currency::Aed => self.aed,
            Currency::Sar => self.sar,
            Currency::Hkd => self.hkd,
            Currency::Sgd => self.sgd,
            Currency::Chf => self.chf,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProductDocumentValidationError {
    #[error("product sale projection metadata must be complete when present")]
    PartialSaleProjection,
}

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
    pub(crate) source_price: Option<SourcePriceDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) sale_prices: Option<SalePricesDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) sale_fx_rate_id: Option<FxRateId>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub(crate) sold_at: Option<OffsetDateTime>,
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

    pub(crate) fn validate(&self) -> Result<(), ProductDocumentValidationError> {
        match (&self.sale_prices, self.sale_fx_rate_id, self.sold_at) {
            (None, None, None) | (None, Some(_), Some(_)) | (Some(_), Some(_), Some(_)) => Ok(()),
            _ => Err(ProductDocumentValidationError::PartialSaleProjection),
        }
    }

    pub(crate) fn source_price(&self) -> Option<(u64, Currency)> {
        self.source_price
            .as_ref()
            .map(|price| (price.amount, Currency::from(price.currency)))
    }

    pub(crate) fn sale_price(&self, currency: Currency) -> Option<u64> {
        self.sale_prices
            .as_ref()
            .map(|prices| prices.amount_in(currency))
    }

    pub(crate) fn has_sale_valuation(&self) -> bool {
        self.sale_fx_rate_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            structured_address_addressline: None,
            structured_address_addressline_extra: None,
            structured_address_locality: None,
            structured_address_region: None,
            structured_address_postal_code: None,
            structured_address_country: None,
            structured_address_continent: None,
            geo_address: None,
            title: TextDocument::new("Vase", LanguageDocument::En),
            title_de: None,
            title_en: Some("Vase".to_owned()),
            title_fr: None,
            title_es: None,
            title_it: None,
            source_price: Some(SourcePriceDocument {
                amount: 100,
                currency: CurrencyDocument::Eur,
            }),
            sale_prices: None,
            sale_fx_rate_id: None,
            sold_at: None,
            state: ProductStateDocument::Available,
            lifecycle: ProductLifecycleDocument::Active,
            url: Url::parse("https://shop.example/products/sku-1")?,
            view_url: Url::parse("https://aura.example/products/vase-abcdef")?,
            images: IndexSet::new(),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        })
    }

    #[test]
    fn should_serialize_only_source_price_for_active_product()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(document()?)?;

        assert_eq!(
            Some(&serde_json::json!(100)),
            value.pointer("/sourcePrice/amount")
        );
        assert_eq!(
            Some(&serde_json::json!("EUR")),
            value.pointer("/sourcePrice/currency")
        );
        assert!(value.get("salePrices").is_none());
        assert!(value.get("priceEur").is_none());
        assert!(value.get("priceUsd").is_none());
        assert!(value.get("priceEstimateMinEur").is_none());
        assert!(value.get("priceEstimateMaxUsd").is_none());
        assert!(value.as_object().is_some_and(|fields| {
            fields.keys().all(|field| {
                !field.starts_with("priceEstimate") && field != "priceEur" && field != "priceUsd"
            })
        }));
        Ok(())
    }

    #[test]
    fn should_allow_sale_metadata_without_sale_prices() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.source_price = None;
        document.sale_fx_rate_id = Some(FxRateId::new());
        document.sold_at = Some(OffsetDateTime::UNIX_EPOCH);

        assert_eq!(Ok(()), document.validate());
        Ok(())
    }

    #[test]
    fn should_reject_partial_sale_projection() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.sale_fx_rate_id = Some(FxRateId::new());

        assert_eq!(
            Err(ProductDocumentValidationError::PartialSaleProjection),
            document.validate()
        );
        Ok(())
    }

    #[test]
    fn should_reject_incomplete_sale_prices_during_deserialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(document()?)?;
        value["salePrices"] = serde_json::json!({ "eur": 100 });
        value["saleFxRateId"] = serde_json::json!(FxRateId::new());
        value["soldAt"] = serde_json::json!("1970-01-01T00:00:00Z");

        assert!(serde_json::from_value::<ProductDocument>(value).is_err());
        Ok(())
    }
}
