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
use common::currency::domain::Currency;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::language::document::TextDocument;
use common::language::domain::Language;
use common::localized::Localized;
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
    pub description_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_es: Option<String>,

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

    pub authenticity: AuthenticityDocument,
    pub condition: ConditionDocument,
    pub provenance: ProvenanceDocument,
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
            description_de: event_product_document.description_de,
            description_en: event_product_document.description_en,
            description_fr: event_product_document.description_fr,
            description_es: event_product_document.description_es,
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
            title_native: product_document.title_native.into(),
            title_de: product_document.title_de,
            title_en: product_document.title_en,
            title_fr: product_document.title_fr,
            title_es: product_document.title_es,
            description_de: product_document.description_de,
            description_en: product_document.description_en,
            description_fr: product_document.description_fr,
            description_es: product_document.description_es,
            price_eur: product_document.price_eur,
            price_usd: product_document.price_gbp,
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
        ]
        .into()
    }
}

impl From<ProductDocument> for Product {
    fn from(product_document: ProductDocument) -> Self {
        let mut other_title = HashMap::with_capacity(2);
        if let Some(title_en) = product_document.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = product_document.title_de {
            other_title.insert(Language::De, title_de.into());
        }

        let mut other_description = HashMap::with_capacity(2);
        if let Some(description_en) = product_document.description_en {
            other_description.insert(Language::En, description_en.into());
        }
        if let Some(description_de) = product_document.description_de {
            other_description.insert(Language::De, description_de.into());
        }

        let mut other_price = HashMap::with_capacity(2);
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

        let mut other_price_estimate_min = HashMap::with_capacity(2);
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

        let mut other_price_estimate_max = HashMap::with_capacity(2);
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
                shop_name,
                shop_type: config.fake_with_rng(rng),
                title_native,
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_fr: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_es: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
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
