use crate::core::origin_year::OriginYear;
use crate::core::product::Product;
use crate::core::product_image::ProductImage;
use crate::dynamodb::authenticity_record::AuthenticityRecord;
use crate::dynamodb::condition_record::ConditionRecord;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_image_record::ProductImageRecord;
use crate::dynamodb::product_state_record::ProductStateRecord;
use crate::dynamodb::provenance_record::ProvenanceRecord;
use crate::dynamodb::restoration_record::RestorationRecord;
use common::category_key::CategoryId;
use common::currency::domain::Currency;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::localized::Localized;
use common::period_key::PeriodId;
use common::price::domain::Price;
use common::price::record::PriceRecord;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use common::year::{Year, YearRange};
use field::field;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashMap;
use strum::EnumCount;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductRecord {
    pub pk: String,
    pub sk: String,
    pub gsi2_pk: String,
    pub gsi2_sk: String,

    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub seller_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub seller_name: String,
    pub shop_type: ShopTypeRecord,
    pub category_id: Option<CategoryId>,
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

    pub title_native: TextRecord,
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
    pub description_native: Option<TextRecord>,
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
    pub price_native: Option<PriceRecord>,
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
    pub price_estimate_min_native: Option<PriceRecord>,
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
    pub price_estimate_max_native: Option<PriceRecord>,
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

    pub state: ProductStateRecord,
    pub url: Url,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<ProductImageRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_max: Option<Year>,

    #[serde(default)]
    pub authenticity: AuthenticityRecord,
    #[serde(default)]
    pub condition: ConditionRecord,
    #[serde(default)]
    pub provenance: ProvenanceRecord,
    #[serde(default)]
    pub restoration: RestorationRecord,

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

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk() -> &'static str {
    "product#materialized"
}

pub fn mk_gsi2_pk(shop_slug_id: &SlugId<0>, product_slug_id: &SlugId<6>) -> String {
    format!("shop_slug_id#{shop_slug_id}#product_slug_id#{product_slug_id}")
}

pub fn mk_gsi2_sk() -> &'static str {
    "product#lookup#shop_id#shops_product_id"
}

impl HasKey for ProductRecord {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl From<ProductRecord> for Product {
    fn from(record: ProductRecord) -> Self {
        let mut category_name = HashMap::with_capacity(Language::COUNT);
        if let Some(category_en) = record.category_name_en {
            category_name.insert(Language::En, category_en.into());
        }
        if let Some(category_de) = record.category_name_de {
            category_name.insert(Language::De, category_de.into());
        }
        if let Some(category_fr) = record.category_name_fr {
            category_name.insert(Language::Fr, category_fr.into());
        }
        if let Some(category_es) = record.category_name_es {
            category_name.insert(Language::Es, category_es.into());
        }
        if let Some(category_it) = record.category_name_it {
            category_name.insert(Language::It, category_it.into());
        }
        let mut period_name = HashMap::with_capacity(Language::COUNT);
        if let Some(period_en) = record.period_name_en {
            period_name.insert(Language::En, period_en.into());
        }
        if let Some(period_de) = record.period_name_de {
            period_name.insert(Language::De, period_de.into());
        }
        if let Some(period_fr) = record.period_name_fr {
            period_name.insert(Language::Fr, period_fr.into());
        }
        if let Some(period_es) = record.period_name_es {
            period_name.insert(Language::Es, period_es.into());
        }
        if let Some(period_it) = record.period_name_it {
            period_name.insert(Language::It, period_it.into());
        }

        let mut other_title = HashMap::with_capacity(Language::COUNT);
        if let Some(title_en) = record.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = record.title_de {
            other_title.insert(Language::De, title_de.into());
        }
        if let Some(title_fr) = record.title_fr {
            other_title.insert(Language::Fr, title_fr.into());
        }
        if let Some(title_es) = record.title_es {
            other_title.insert(Language::Es, title_es.into());
        }
        if let Some(title_it) = record.title_it {
            other_title.insert(Language::It, title_it.into());
        }

        let mut other_description = HashMap::with_capacity(Language::COUNT);
        if let Some(description_en) = record.description_en {
            other_description.insert(Language::En, description_en.into());
        }
        if let Some(description_de) = record.description_de {
            other_description.insert(Language::De, description_de.into());
        }
        if let Some(description_fr) = record.description_fr {
            other_description.insert(Language::Fr, description_fr.into());
        }
        if let Some(description_es) = record.description_es {
            other_description.insert(Language::Es, description_es.into());
        }
        if let Some(description_it) = record.description_it {
            other_description.insert(Language::It, description_it.into());
        }

        let mut other_price = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = record.price_eur {
            other_price.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_gbp {
            other_price.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_usd {
            other_price.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_aud {
            other_price.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_cad {
            other_price.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_nzd {
            other_price.insert(Currency::Nzd, price_eur.into());
        }

        let mut other_price_estimate_min = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = record.price_estimate_min_eur {
            other_price_estimate_min.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_gbp {
            other_price_estimate_min.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_usd {
            other_price_estimate_min.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_aud {
            other_price_estimate_min.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_cad {
            other_price_estimate_min.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_nzd {
            other_price_estimate_min.insert(Currency::Nzd, price_eur.into());
        }

        let mut other_price_estimate_max = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = record.price_estimate_max_eur {
            other_price_estimate_max.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_gbp {
            other_price_estimate_max.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_usd {
            other_price_estimate_max.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_aud {
            other_price_estimate_max.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_cad {
            other_price_estimate_max.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_nzd {
            other_price_estimate_max.insert(Currency::Nzd, price_eur.into());
        }

        Product {
            product_id: record.product_id,
            product_slug_id: record.product_slug_id,
            shop_slug_id: record.shop_slug_id,
            seller_slug_id: record.seller_slug_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            seller_id: record.seller_id,
            shops_product_id: record.shops_product_id,
            shop_name: record.shop_name.into(),
            seller_name: record.seller_name.into(),
            shop_type: record.shop_type.into(),
            category_id: record.category_id,
            category_name,
            period_id: record.period_id,
            period_name,
            native_title: Localized::new(
                record.title_native.language.into(),
                record.title_native.text.into(),
            ),
            other_title,
            native_description: record.description_native.map(|text_record| {
                Localized::new(text_record.language.into(), text_record.text.into())
            }),
            other_description,
            native_price: record.price_native.map(Price::from),
            other_price,
            native_price_estimate_min: record.price_estimate_min_native.map(Price::from),
            other_price_estimate_min,
            native_price_estimate_max: record.price_estimate_max_native.map(Price::from),
            other_price_estimate_max,
            state: record.state.into(),
            url: record.url,
            images: record.images.into_iter().map(ProductImage::from).collect(),
            embedding: record.embedding,
            origin_year: match record.origin_year {
                Some(exact_year) => Some(OriginYear::ExactYear(exact_year)),
                None => match (record.origin_year_min, record.origin_year_max) {
                    (None, None) => None,
                    (min, max) => Some(OriginYear::EstimatedRange(YearRange { min, max })),
                },
            },
            authenticity: record.authenticity.into(),
            condition: record.condition.into(),
            provenance: record.provenance.into(),
            restoration: record.restoration.into(),
            auction_start: record.auction_start,
            auction_end: record.auction_end,
            created: record.created,
            updated: record.updated,
        }
    }
}

impl TryFrom<ProductDomainEventRecord> for ProductRecord {
    type Error = PersistenceMappingError;

    fn try_from(event_record: ProductDomainEventRecord) -> Result<Self, Self::Error> {
        let product_slug_id = event_record.product_slug_id.ok_or_else(|| {
            MissingPersistenceField::new(field!(product_slug_id@ProductDomainEventRecord))
        })?;
        let shop_slug_id = event_record.shop_slug_id.ok_or_else(|| {
            MissingPersistenceField::new(field!(shop_slug_id@ProductDomainEventRecord))
        })?;
        let seller_slug_id = event_record.seller_slug_id.ok_or_else(|| {
            MissingPersistenceField::new(field!(seller_slug_id@ProductDomainEventRecord))
        })?;
        let record = ProductRecord {
            pk: event_record.pk,
            sk: mk_sk().to_string(),
            gsi2_pk: mk_gsi2_pk(&shop_slug_id, &product_slug_id),
            gsi2_sk: mk_gsi2_sk().to_owned(),
            product_id: event_record.product_id,
            product_slug_id,
            shop_slug_id,
            seller_slug_id,
            event_id: event_record.event_id,
            shop_id: event_record.shop_id,
            seller_id: event_record.seller_id,
            shops_product_id: event_record.shops_product_id,
            shop_name: event_record.shop_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@ProductDomainEventRecord))
            })?,
            seller_name: event_record.seller_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(seller_name@ProductDomainEventRecord))
            })?,
            shop_type: event_record.shop_type.ok_or_else(|| {
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
            title_native: event_record.title_native.ok_or_else(|| {
                MissingPersistenceField::new(field!(title_native@ProductDomainEventRecord))
            })?,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            title_fr: event_record.title_fr,
            title_es: event_record.title_es,
            title_it: event_record.title_it,
            description_native: event_record.description_native,
            description_de: event_record.description_de,
            description_en: event_record.description_en,
            description_fr: event_record.description_fr,
            description_es: event_record.description_es,
            description_it: event_record.description_it,
            price_native: event_record.new_price_native,
            price_eur: event_record.new_price_eur,
            price_usd: event_record.new_price_usd,
            price_gbp: event_record.new_price_gbp,
            price_aud: event_record.new_price_aud,
            price_cad: event_record.new_price_cad,
            price_nzd: event_record.new_price_nzd,
            price_estimate_min_native: event_record.new_price_estimate_min_native,
            price_estimate_min_eur: event_record.new_price_estimate_min_eur,
            price_estimate_min_usd: event_record.new_price_estimate_min_usd,
            price_estimate_min_gbp: event_record.new_price_estimate_min_gbp,
            price_estimate_min_aud: event_record.new_price_estimate_min_aud,
            price_estimate_min_cad: event_record.new_price_estimate_min_cad,
            price_estimate_min_nzd: event_record.new_price_estimate_min_nzd,
            price_estimate_max_native: event_record.new_price_estimate_max_native,
            price_estimate_max_eur: event_record.new_price_estimate_max_eur,
            price_estimate_max_usd: event_record.new_price_estimate_max_usd,
            price_estimate_max_gbp: event_record.new_price_estimate_max_gbp,
            price_estimate_max_aud: event_record.new_price_estimate_max_aud,
            price_estimate_max_cad: event_record.new_price_estimate_max_cad,
            price_estimate_max_nzd: event_record.new_price_estimate_max_nzd,
            state: event_record.new_state.ok_or_else(|| {
                MissingPersistenceField::new(field!(new_state@ProductDomainEventRecord))
            })?,
            url: event_record.url.ok_or_else(|| {
                MissingPersistenceField::new(field!(url@ProductDomainEventRecord))
            })?,
            images: event_record.images.unwrap_or_default(),
            embedding: None,
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: Default::default(),
            condition: Default::default(),
            provenance: Default::default(),
            restoration: Default::default(),
            auction_start: event_record.auction_start,
            auction_end: event_record.auction_end,
            created: event_record.timestamp,
            updated: event_record.timestamp,
        };

        Ok(record)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::description::Description;
    use crate::core::title::Title;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let now = OffsetDateTime::now_utc();
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_product_id: ShopsProductId = config.fake_with_rng(rng);
            let price_native: Option<PriceRecord> =
                Some(config.fake_with_rng::<Price, _>(rng).into());
            let state: ProductStateRecord = config.fake_with_rng(rng);
            let origin_year_min = fake::rand::random_range(1807..=1815).into();
            let origin_year_max = fake::rand::random_range(1815..=1819).into();
            let origin_year = if origin_year_min == origin_year_max {
                Some(origin_year_min)
            } else {
                None
            };

            let title_native = TextRecord::new(
                config.fake_with_rng::<Title, _>(rng).to_string(),
                config.fake_with_rng(rng),
            );
            let shop_name = config.fake_with_rng(rng);
            let seller_name = config.fake_with_rng(rng);
            let shop_slug_id = SlugId::from(&shop_name);
            let seller_slug_id = SlugId::from(&seller_name);
            let product_slug_id = SlugId::from(&title_native.text);
            ProductRecord {
                pk: mk_pk(&shop_id, &shops_product_id),
                sk: mk_sk().to_string(),
                gsi2_pk: mk_gsi2_pk(&shop_slug_id, &product_slug_id),
                gsi2_sk: mk_gsi2_sk().to_owned(),
                product_id: config.fake_with_rng(rng),
                product_slug_id,
                shop_slug_id,
                seller_slug_id,
                event_id: config.fake_with_rng(rng),
                shop_id,
                seller_id: config.fake_with_rng(rng),
                shops_product_id: shops_product_id.clone(),
                shop_name,
                seller_name,
                shop_type: config.fake_with_rng(rng),
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
                title_native,
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_it: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                description_native: Some(TextRecord::new(
                    config.fake_with_rng::<Description, _>(rng).to_string(),
                    config.fake_with_rng(rng),
                )),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_fr: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_es: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_it: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                price_native,
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_native: Some(config.fake_with_rng::<Price, _>(rng).into()),
                price_estimate_min_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_native: Some(config.fake_with_rng::<Price, _>(rng).into()),
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
                embedding: if config.fake_with_rng(rng) {
                    Some(fake::vec![f32; 768])
                } else {
                    None
                },
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
                created: now,
                updated: now,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_record::ProductRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_product_record() {
            let _ = Faker.fake::<ProductRecord>();
        }
    }
}
