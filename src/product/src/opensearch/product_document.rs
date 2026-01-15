use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_record::ProductRecord;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
use crate::opensearch::product_image_document::ProductImageDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::provenance_document::ProvenanceDocument;
use crate::opensearch::restoration_document::RestorationDocument;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::language::document::TextDocument;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::year::Year;
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticity: Option<AuthenticityDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<ConditionDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provenance: Option<ProvenanceDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub restoration: Option<RestorationDocument>,

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

impl TryFrom<ProductEventRecord> for ProductDocument {
    type Error = PersistenceMappingError;

    fn try_from(event_record: ProductEventRecord) -> Result<Self, Self::Error> {
        let state = event_record
            .new_state
            .map(ProductStateDocument::from)
            .ok_or_else(|| MissingPersistenceField::new(field!(new_state@ProductEventRecord)))?;
        let document = ProductDocument {
            product_id: event_record.product_id,
            event_id: event_record.event_id,
            shop_id: event_record.shop_id,
            shops_product_id: event_record.shops_product_id,
            shop_name: event_record.shop_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@ProductEventRecord))
            })?,
            shop_type: event_record.shop_type.map(Into::into).ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_type@ProductEventRecord))
            })?,
            title_native: event_record
                .title_native
                .map(TextDocument::from)
                .ok_or_else(|| {
                    MissingPersistenceField::new(field!(title_native@ProductEventRecord))
                })?,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            title_fr: event_record.title_fr,
            title_es: event_record.title_es,
            description_de: event_record.description_de,
            description_en: event_record.description_en,
            description_fr: event_record.description_fr,
            description_es: event_record.description_es,
            price_eur: event_record.new_price_eur,
            price_usd: event_record.new_price_usd,
            price_gbp: event_record.new_price_gbp,
            price_aud: event_record.new_price_aud,
            price_cad: event_record.new_price_cad,
            price_nzd: event_record.new_price_nzd,
            price_estimate_min_eur: event_record.new_price_estimate_min_eur,
            price_estimate_min_usd: event_record.new_price_estimate_min_usd,
            price_estimate_min_gbp: event_record.new_price_estimate_min_gbp,
            price_estimate_min_aud: event_record.new_price_estimate_min_aud,
            price_estimate_min_cad: event_record.new_price_estimate_min_cad,
            price_estimate_min_nzd: event_record.new_price_estimate_min_nzd,
            price_estimate_max_eur: event_record.new_price_estimate_max_eur,
            price_estimate_max_usd: event_record.new_price_estimate_max_usd,
            price_estimate_max_gbp: event_record.new_price_estimate_max_gbp,
            price_estimate_max_aud: event_record.new_price_estimate_max_aud,
            price_estimate_max_cad: event_record.new_price_estimate_max_cad,
            price_estimate_max_nzd: event_record.new_price_estimate_max_nzd,
            state,
            url: event_record
                .url
                .ok_or_else(|| MissingPersistenceField::new(field!(url@ProductEventRecord)))?,
            images: event_record
                .images
                .unwrap_or_default()
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            text_embedding: None,
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            auction_start: event_record.auction_start,
            auction_end: event_record.auction_end,
            created: event_record.timestamp,
            updated: event_record.timestamp,
        };
        Ok(document)
    }
}

impl From<ProductRecord> for ProductDocument {
    fn from(record: ProductRecord) -> Self {
        ProductDocument {
            product_id: record.product_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id,
            shop_name: record.shop_name,
            shop_type: record.shop_type.into(),
            title_native: record.title_native.into(),
            title_de: record.title_de,
            title_en: record.title_en,
            title_fr: record.title_fr,
            title_es: record.title_es,
            description_de: record.description_de,
            description_en: record.description_en,
            description_fr: record.description_fr,
            description_es: record.description_es,
            price_eur: record.price_eur,
            price_usd: record.price_gbp,
            price_gbp: record.price_gbp,
            price_aud: record.price_aud,
            price_cad: record.price_cad,
            price_nzd: record.price_nzd,
            price_estimate_min_eur: record.price_estimate_min_eur,
            price_estimate_min_usd: record.price_estimate_min_usd,
            price_estimate_min_gbp: record.price_estimate_min_gbp,
            price_estimate_min_aud: record.price_estimate_min_aud,
            price_estimate_min_cad: record.price_estimate_min_cad,
            price_estimate_min_nzd: record.price_estimate_min_nzd,
            price_estimate_max_eur: record.price_estimate_max_eur,
            price_estimate_max_usd: record.price_estimate_max_usd,
            price_estimate_max_gbp: record.price_estimate_max_gbp,
            price_estimate_max_aud: record.price_estimate_max_aud,
            price_estimate_max_cad: record.price_estimate_max_cad,
            price_estimate_max_nzd: record.price_estimate_max_nzd,
            state: record.state.into(),
            url: record.url,
            images: record
                .images
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            text_embedding: None,
            origin_year_min: record.origin_year_min,
            origin_year: record.origin_year,
            origin_year_max: record.origin_year_max,
            authenticity: record.authenticity.map(AuthenticityDocument::from),
            condition: record.condition.map(ConditionDocument::from),
            provenance: record.provenance.map(ProvenanceDocument::from),
            restoration: record.restoration.map(RestorationDocument::from),
            auction_start: record.auction_start,
            auction_end: record.auction_end,
            created: record.created,
            updated: record.updated,
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

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::description::Description;
    use crate::core::title::Title;
    use common::price::domain::MonetaryAmount;
    use common::shop_name::ShopName;
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
            ProductDocument {
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng::<ShopName, _>(rng).into(),
                shop_type: config.fake_with_rng(rng),
                title_native: TextDocument {
                    text: config.fake_with_rng::<Title, _>(rng).to_string(),
                    language: config.fake_with_rng(rng),
                },
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
                provenance: if config.fake_with_rng(rng) {
                    Some(config.fake_with_rng(rng))
                } else {
                    None
                },
                restoration: if config.fake_with_rng(rng) {
                    Some(config.fake_with_rng(rng))
                } else {
                    None
                },
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
