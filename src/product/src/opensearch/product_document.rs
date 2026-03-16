use std::collections::HashMap;

use crate::core::origin_year::OriginYear;
use crate::core::product::Product;
use crate::core::product_image::ProductImage;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_record::ProductRecord;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
use crate::opensearch::product_image_document::ProductImageDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::provenance_document::ProvenanceDocument;
use crate::opensearch::restoration_document::RestorationDocument;
use common::category_key::CategoryId;
use common::currency::domain::Currency;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::language::document::TextDocument;
use common::language::domain::Language;
use common::localized::Localized;
use common::period_key::PeriodId;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use common::year::{Year, YearRange};
use common::{event_id::EventId, has_key::HasKey};
use field::field;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::opensearch::shop_type_document::ShopTypeDocument;
use strum::EnumCount;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ProductDocument {
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub shop_type: ShopTypeDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_id: Option<CategoryId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_id: Option<PeriodId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_it: Option<String>,

    pub title_native: TextDocument,
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
    pub description_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_it: Option<String>,

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

    pub state: ProductStateDocument,
    pub url: Url,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<ProductImageDocument>,

    // title [SEP] description, dim=1024 via baai/bge-m3
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_embedding: Option<Vec<f32>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_max: Option<Year>,

    #[serde(default)]
    pub authenticity: AuthenticityDocument,
    #[serde(default)]
    pub condition: ConditionDocument,
    #[serde(default)]
    pub provenance: ProvenanceDocument,
    #[serde(default)]
    pub restoration: RestorationDocument,

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
    pub fn _id(&self) -> ProductId {
        self.product_id
    }
}

impl HasKey for ProductDocument {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl TryFrom<ProductDomainEventRecord> for ProductDocument {
    type Error = PersistenceMappingError;

    fn try_from(event_product_document: ProductDomainEventRecord) -> Result<Self, Self::Error> {
        let state = event_product_document
            .new_state
            .map(ProductStateDocument::from)
            .ok_or_else(|| {
                MissingPersistenceField::new(field!(new_state@ProductDomainEventRecord))
            })?;
        let document = ProductDocument {
            product_id: event_product_document.product_id,
            product_slug_id: event_product_document.product_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(product_slug_id@ProductDomainEventRecord))
            })?,
            shop_slug_id: event_product_document.shop_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_slug_id@ProductDomainEventRecord))
            })?,
            event_id: event_product_document.event_id,
            shop_id: event_product_document.shop_id,
            shops_product_id: event_product_document.shops_product_id,
            shop_name: event_product_document.shop_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@ProductDomainEventRecord))
            })?,
            shop_type: event_product_document
                .shop_type
                .map(Into::into)
                .ok_or_else(|| {
                    MissingPersistenceField::new(field!(shop_type@ProductDomainEventRecord))
                })?,
            category_id: None,
            period_id: None,
            category_name_de: None,
            category_name_en: None,
            category_name_fr: None,
            category_name_es: None,
            category_name_it: None,
            period_name_de: None,
            period_name_en: None,
            period_name_fr: None,
            period_name_es: None,
            period_name_it: None,
            title_native: event_product_document
                .title_native
                .map(TextDocument::from)
                .ok_or_else(|| {
                    MissingPersistenceField::new(field!(title_native@ProductDomainEventRecord))
                })?,
            title_de: event_product_document.title_de,
            title_en: event_product_document.title_en,
            title_fr: event_product_document.title_fr,
            title_es: event_product_document.title_es,
            title_it: event_product_document.title_it,
            description_de: event_product_document.description_de,
            description_en: event_product_document.description_en,
            description_fr: event_product_document.description_fr,
            description_es: event_product_document.description_es,
            description_it: event_product_document.description_it,
            price_eur: event_product_document.new_price_eur,
            price_usd: event_product_document.new_price_usd,
            price_gbp: event_product_document.new_price_gbp,
            price_aud: event_product_document.new_price_aud,
            price_cad: event_product_document.new_price_cad,
            price_nzd: event_product_document.new_price_nzd,
            price_estimate_min_eur: event_product_document.new_price_estimate_min_eur,
            price_estimate_min_usd: event_product_document.new_price_estimate_min_usd,
            price_estimate_min_gbp: event_product_document.new_price_estimate_min_gbp,
            price_estimate_min_aud: event_product_document.new_price_estimate_min_aud,
            price_estimate_min_cad: event_product_document.new_price_estimate_min_cad,
            price_estimate_min_nzd: event_product_document.new_price_estimate_min_nzd,
            price_estimate_max_eur: event_product_document.new_price_estimate_max_eur,
            price_estimate_max_usd: event_product_document.new_price_estimate_max_usd,
            price_estimate_max_gbp: event_product_document.new_price_estimate_max_gbp,
            price_estimate_max_aud: event_product_document.new_price_estimate_max_aud,
            price_estimate_max_cad: event_product_document.new_price_estimate_max_cad,
            price_estimate_max_nzd: event_product_document.new_price_estimate_max_nzd,
            state,
            url: event_product_document.url.ok_or_else(|| {
                MissingPersistenceField::new(field!(url@ProductDomainEventRecord))
            })?,
            images: event_product_document
                .images
                .unwrap_or_default()
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            text_embedding: None,
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: Default::default(),
            condition: Default::default(),
            provenance: Default::default(),
            restoration: Default::default(),
            auction_start: event_product_document.auction_start,
            auction_end: event_product_document.auction_end,
            created: event_product_document.timestamp,
            updated: event_product_document.timestamp,
        };
        Ok(document)
    }
}

impl From<ProductRecord> for ProductDocument {
    fn from(product_document: ProductRecord) -> Self {
        ProductDocument {
            product_id: product_document.product_id,
            product_slug_id: product_document.product_slug_id,
            shop_slug_id: product_document.shop_slug_id,
            event_id: product_document.event_id,
            shop_id: product_document.shop_id,
            shops_product_id: product_document.shops_product_id,
            shop_name: product_document.shop_name,
            shop_type: product_document.shop_type.into(),
            category_id: product_document.category_id,
            period_id: product_document.period_id,
            category_name_de: product_document.category_name_de,
            category_name_en: product_document.category_name_en,
            category_name_fr: product_document.category_name_fr,
            category_name_es: product_document.category_name_es,
            category_name_it: product_document.category_name_it,
            period_name_de: product_document.period_name_de,
            period_name_en: product_document.period_name_en,
            period_name_fr: product_document.period_name_fr,
            period_name_es: product_document.period_name_es,
            period_name_it: product_document.period_name_it,
            title_native: product_document.title_native.into(),
            title_de: product_document.title_de,
            title_en: product_document.title_en,
            title_fr: product_document.title_fr,
            title_es: product_document.title_es,
            title_it: product_document.title_it,
            description_de: product_document.description_de,
            description_en: product_document.description_en,
            description_fr: product_document.description_fr,
            description_es: product_document.description_es,
            description_it: product_document.description_it,
            price_eur: product_document.price_eur,
            price_usd: product_document.price_usd,
            price_gbp: product_document.price_gbp,
            price_aud: product_document.price_aud,
            price_cad: product_document.price_cad,
            price_nzd: product_document.price_nzd,
            price_estimate_min_eur: product_document.price_estimate_min_eur,
            price_estimate_min_usd: product_document.price_estimate_min_usd,
            price_estimate_min_gbp: product_document.price_estimate_min_gbp,
            price_estimate_min_aud: product_document.price_estimate_min_aud,
            price_estimate_min_cad: product_document.price_estimate_min_cad,
            price_estimate_min_nzd: product_document.price_estimate_min_nzd,
            price_estimate_max_eur: product_document.price_estimate_max_eur,
            price_estimate_max_usd: product_document.price_estimate_max_usd,
            price_estimate_max_gbp: product_document.price_estimate_max_gbp,
            price_estimate_max_aud: product_document.price_estimate_max_aud,
            price_estimate_max_cad: product_document.price_estimate_max_cad,
            price_estimate_max_nzd: product_document.price_estimate_max_nzd,
            state: product_document.state.into(),
            url: product_document.url,
            images: product_document
                .images
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            text_embedding: None,
            origin_year_min: product_document.origin_year_min,
            origin_year: product_document.origin_year,
            origin_year_max: product_document.origin_year_max,
            authenticity: product_document.authenticity.into(),
            condition: product_document.condition.into(),
            provenance: product_document.provenance.into(),
            restoration: product_document.restoration.into(),
            auction_start: product_document.auction_start,
            auction_end: product_document.auction_end,
            created: product_document.created,
            updated: product_document.updated,
        }
    }
}

impl ProductDocumentSerdeField {
    pub fn description_fields() -> Vec<ProductDocumentSerdeField> {
        [
            ProductDocumentSerdeField::DescriptionDe,
            ProductDocumentSerdeField::DescriptionEn,
            ProductDocumentSerdeField::DescriptionFr,
            ProductDocumentSerdeField::DescriptionEs,
            ProductDocumentSerdeField::DescriptionIt,
        ]
        .into()
    }
}

fn extract_price(
    native: &Option<common::price::domain::Price>,
    other: &HashMap<Currency, common::price::domain::MonetaryAmount>,
    currency: Currency,
) -> Option<u64> {
    if let Some(amount) = other.get(&currency) {
        return Some(u64::from(*amount));
    }
    if let Some(price) = native {
        if price.currency == currency {
            return Some(u64::from(price.monetary_amount));
        }
    }
    None
}

impl From<Product> for ProductDocument {
    fn from(product: Product) -> Self {
        use crate::core::description::Description;
        use crate::core::title::Title;

        let category_name_de = product.category_name.get(&Language::De).map(|c| String::from(c.clone()));
        let category_name_en = product.category_name.get(&Language::En).map(|c| String::from(c.clone()));
        let category_name_fr = product.category_name.get(&Language::Fr).map(|c| String::from(c.clone()));
        let category_name_es = product.category_name.get(&Language::Es).map(|c| String::from(c.clone()));
        let category_name_it = product.category_name.get(&Language::It).map(|c| String::from(c.clone()));

        let period_name_de = product.period_name.get(&Language::De).map(|c| String::from(c.clone()));
        let period_name_en = product.period_name.get(&Language::En).map(|c| String::from(c.clone()));
        let period_name_fr = product.period_name.get(&Language::Fr).map(|c| String::from(c.clone()));
        let period_name_es = product.period_name.get(&Language::Es).map(|c| String::from(c.clone()));
        let period_name_it = product.period_name.get(&Language::It).map(|c| String::from(c.clone()));

        let title_de = product.other_title.get(&Language::De).map(|t| String::from(t.clone()));
        let title_en = product.other_title.get(&Language::En).map(|t| String::from(t.clone()));
        let title_fr = product.other_title.get(&Language::Fr).map(|t| String::from(t.clone()));
        let title_es = product.other_title.get(&Language::Es).map(|t| String::from(t.clone()));
        let title_it = product.other_title.get(&Language::It).map(|t| String::from(t.clone()));

        let description_de = product.other_description.get(&Language::De).map(|d| String::from(d.clone()));
        let description_en = product.other_description.get(&Language::En).map(|d| String::from(d.clone()));
        let description_fr = product.other_description.get(&Language::Fr).map(|d| String::from(d.clone()));
        let description_es = product.other_description.get(&Language::Es).map(|d| String::from(d.clone()));
        let description_it = product.other_description.get(&Language::It).map(|d| String::from(d.clone()));

        let (origin_year_min, origin_year, origin_year_max) = match product.origin_year {
            Some(OriginYear::ExactYear(y)) => (None, Some(y), None),
            Some(OriginYear::EstimatedRange(range)) => (range.min, None, range.max),
            None => (None, None, None),
        };

        ProductDocument {
            product_id: product.product_id,
            product_slug_id: product.product_slug_id,
            shop_slug_id: product.shop_slug_id,
            event_id: product.event_id,
            shop_id: product.shop_id,
            shops_product_id: product.shops_product_id,
            shop_name: String::from(product.shop_name),
            shop_type: product.shop_type.into(),
            category_id: product.category_id,
            period_id: product.period_id,
            category_name_de,
            category_name_en,
            category_name_fr,
            category_name_es,
            category_name_it,
            period_name_de,
            period_name_en,
            period_name_fr,
            period_name_es,
            period_name_it,
            title_native: product.native_title.into(),
            title_de,
            title_en,
            title_fr,
            title_es,
            title_it,
            description_de,
            description_en,
            description_fr,
            description_es,
            description_it,
            price_eur: extract_price(&product.native_price, &product.other_price, Currency::Eur),
            price_usd: extract_price(&product.native_price, &product.other_price, Currency::Usd),
            price_gbp: extract_price(&product.native_price, &product.other_price, Currency::Gbp),
            price_aud: extract_price(&product.native_price, &product.other_price, Currency::Aud),
            price_cad: extract_price(&product.native_price, &product.other_price, Currency::Cad),
            price_nzd: extract_price(&product.native_price, &product.other_price, Currency::Nzd),
            price_estimate_min_eur: extract_price(&product.native_price_estimate_min, &product.other_price_estimate_min, Currency::Eur),
            price_estimate_min_usd: extract_price(&product.native_price_estimate_min, &product.other_price_estimate_min, Currency::Usd),
            price_estimate_min_gbp: extract_price(&product.native_price_estimate_min, &product.other_price_estimate_min, Currency::Gbp),
            price_estimate_min_aud: extract_price(&product.native_price_estimate_min, &product.other_price_estimate_min, Currency::Aud),
            price_estimate_min_cad: extract_price(&product.native_price_estimate_min, &product.other_price_estimate_min, Currency::Cad),
            price_estimate_min_nzd: extract_price(&product.native_price_estimate_min, &product.other_price_estimate_min, Currency::Nzd),
            price_estimate_max_eur: extract_price(&product.native_price_estimate_max, &product.other_price_estimate_max, Currency::Eur),
            price_estimate_max_usd: extract_price(&product.native_price_estimate_max, &product.other_price_estimate_max, Currency::Usd),
            price_estimate_max_gbp: extract_price(&product.native_price_estimate_max, &product.other_price_estimate_max, Currency::Gbp),
            price_estimate_max_aud: extract_price(&product.native_price_estimate_max, &product.other_price_estimate_max, Currency::Aud),
            price_estimate_max_cad: extract_price(&product.native_price_estimate_max, &product.other_price_estimate_max, Currency::Cad),
            price_estimate_max_nzd: extract_price(&product.native_price_estimate_max, &product.other_price_estimate_max, Currency::Nzd),
            state: product.state.into(),
            url: product.url,
            images: product.images.into_iter().map(ProductImageDocument::from).collect(),
            text_embedding: product.text_embedding,
            origin_year_min,
            origin_year,
            origin_year_max,
            authenticity: product.authenticity.into(),
            condition: product.condition.into(),
            provenance: product.provenance.into(),
            restoration: product.restoration.into(),
            auction_start: product.auction_start,
            auction_end: product.auction_end,
            created: product.created,
            updated: product.updated,
        }
    }
}

impl From<ProductDocument> for Product {
    fn from(product_document: ProductDocument) -> Self {
        let mut category_name = HashMap::with_capacity(Language::COUNT);
        if let Some(category_en) = product_document.category_name_en {
            category_name.insert(Language::En, category_en.into());
        }
        if let Some(category_de) = product_document.category_name_de {
            category_name.insert(Language::De, category_de.into());
        }
        if let Some(category_fr) = product_document.category_name_fr {
            category_name.insert(Language::Fr, category_fr.into());
        }
        if let Some(category_es) = product_document.category_name_es {
            category_name.insert(Language::Es, category_es.into());
        }
        if let Some(category_it) = product_document.category_name_it {
            category_name.insert(Language::It, category_it.into());
        }
        let mut period_name = HashMap::with_capacity(Language::COUNT);
        if let Some(period_en) = product_document.period_name_en {
            period_name.insert(Language::En, period_en.into());
        }
        if let Some(period_de) = product_document.period_name_de {
            period_name.insert(Language::De, period_de.into());
        }
        if let Some(period_fr) = product_document.period_name_fr {
            period_name.insert(Language::Fr, period_fr.into());
        }
        if let Some(period_es) = product_document.period_name_es {
            period_name.insert(Language::Es, period_es.into());
        }
        if let Some(period_it) = product_document.period_name_it {
            period_name.insert(Language::It, period_it.into());
        }

        let mut other_title = HashMap::with_capacity(Language::COUNT);
        if let Some(title_en) = product_document.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = product_document.title_de {
            other_title.insert(Language::De, title_de.into());
        }
        if let Some(title_fr) = product_document.title_fr {
            other_title.insert(Language::Fr, title_fr.into());
        }
        if let Some(title_es) = product_document.title_es {
            other_title.insert(Language::Es, title_es.into());
        }
        if let Some(title_it) = product_document.title_it {
            other_title.insert(Language::It, title_it.into());
        }

        let mut other_description = HashMap::with_capacity(Language::COUNT);
        if let Some(description_en) = product_document.description_en {
            other_description.insert(Language::En, description_en.into());
        }
        if let Some(description_de) = product_document.description_de {
            other_description.insert(Language::De, description_de.into());
        }
        if let Some(description_fr) = product_document.description_fr {
            other_description.insert(Language::Fr, description_fr.into());
        }
        if let Some(description_es) = product_document.description_es {
            other_description.insert(Language::Es, description_es.into());
        }
        if let Some(description_it) = product_document.description_it {
            other_description.insert(Language::It, description_it.into());
        }

        let mut other_price = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = product_document.price_eur {
            other_price.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_gbp {
            other_price.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_usd {
            other_price.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_aud {
            other_price.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_cad {
            other_price.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_nzd {
            other_price.insert(Currency::Nzd, price_eur.into());
        }

        let mut other_price_estimate_min = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = product_document.price_estimate_min_eur {
            other_price_estimate_min.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_gbp {
            other_price_estimate_min.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_usd {
            other_price_estimate_min.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_aud {
            other_price_estimate_min.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_cad {
            other_price_estimate_min.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_nzd {
            other_price_estimate_min.insert(Currency::Nzd, price_eur.into());
        }

        let mut other_price_estimate_max = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = product_document.price_estimate_max_eur {
            other_price_estimate_max.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_gbp {
            other_price_estimate_max.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_usd {
            other_price_estimate_max.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_aud {
            other_price_estimate_max.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_cad {
            other_price_estimate_max.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_nzd {
            other_price_estimate_max.insert(Currency::Nzd, price_eur.into());
        }

        Product {
            product_id: product_document.product_id,
            product_slug_id: product_document.product_slug_id,
            shop_slug_id: product_document.shop_slug_id,
            event_id: product_document.event_id,
            shop_id: product_document.shop_id,
            shops_product_id: product_document.shops_product_id,
            shop_name: product_document.shop_name.into(),
            shop_type: product_document.shop_type.into(),
            category_id: product_document.category_id,
            category_name,
            period_id: product_document.period_id,
            period_name,
            native_title: Localized::new(
                product_document.title_native.language.into(),
                product_document.title_native.text.into(),
            ),
            other_title,
            native_description: None,
            other_description,
            native_price: None,
            other_price,
            native_price_estimate_min: None,
            other_price_estimate_min,
            native_price_estimate_max: None,
            other_price_estimate_max,
            state: product_document.state.into(),
            url: product_document.url,
            images: product_document
                .images
                .into_iter()
                .map(ProductImage::from)
                .collect(),
            text_embedding: product_document.text_embedding,
            origin_year: match product_document.origin_year {
                Some(exact_year) => Some(OriginYear::ExactYear(exact_year)),
                None => match (
                    product_document.origin_year_min,
                    product_document.origin_year_max,
                ) {
                    (None, None) => None,
                    (min, max) => Some(OriginYear::EstimatedRange(YearRange { min, max })),
                },
            },
            authenticity: product_document.authenticity.into(),
            condition: product_document.condition.into(),
            provenance: product_document.provenance.into(),
            restoration: product_document.restoration.into(),
            auction_start: product_document.auction_start,
            auction_end: product_document.auction_end,
            created: product_document.created,
            updated: product_document.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::description::Description;
    use crate::core::title::Title;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let state: ProductStateDocument = config.fake_with_rng(rng);
            let origin_year_min = fake::rand::random_range(1807..=1815).into();
            let origin_year_max = fake::rand::random_range(1815..=1819).into();
            let origin_year = if origin_year_min == origin_year_max {
                Some(origin_year_min)
            } else {
                None
            };
            let title_native = TextDocument {
                text: config.fake_with_rng::<Title, _>(rng).to_string(),
                language: config.fake_with_rng(rng),
            };
            let shop_name: String = config.fake_with_rng(rng);
            ProductDocument {
                product_id: config.fake_with_rng(rng),
                product_slug_id: SlugId::from(&title_native.text),
                shop_slug_id: SlugId::from(&shop_name),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                category_name_de: config.fake_with_rng(rng),
                category_name_en: config.fake_with_rng(rng),
                category_name_fr: config.fake_with_rng(rng),
                category_name_es: config.fake_with_rng(rng),
                category_name_it: config.fake_with_rng(rng),
                period_name_de: config.fake_with_rng(rng),
                period_name_en: config.fake_with_rng(rng),
                period_name_fr: config.fake_with_rng(rng),
                period_name_es: config.fake_with_rng(rng),
                period_name_it: config.fake_with_rng(rng),
                shop_name,
                shop_type: config.fake_with_rng(rng),
                title_native,
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_it: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_fr: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_es: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_it: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: config.fake_with_rng(rng),
                text_embedding: None,
                origin_year_min: Some(origin_year_min),
                origin_year,
                origin_year_max: Some(origin_year_max),
                authenticity: config.fake_with_rng(rng),
                condition: config.fake_with_rng(rng),
                provenance: config.fake_with_rng(rng),
                restoration: config.fake_with_rng(rng),
                auction_start: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                auction_end: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::product_document::ProductDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_document() {
            let _ = Faker.fake::<ProductDocument>();
        }
    }
}
