use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_record::ProductRecord;
use crate::opensearch::product_state_document::ProductStateDocument;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::language::document::TextDocument;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::{event_id::EventId, has_key::HasKey};
use field::field;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
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

    pub state: ProductStateDocument,
    pub url: Url,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<Url>,

    // title [SEP] description, dim=1024 via baai/bge-m3
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_embedding: Option<Vec<f32>>,

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
            state,
            url: event_record
                .url
                .ok_or_else(|| MissingPersistenceField::new(field!(url@ProductEventRecord)))?,
            images: event_record.images.unwrap_or_default(),
            text_embedding: None,
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
            state: record.state.into(),
            url: record.url,
            images: record.images,
            text_embedding: None,
            created: record.created,
            updated: record.updated,
        }
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
            ProductDocument {
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng::<ShopName, _>(rng).into(),
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
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: vec![
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ],
                text_embedding: None,
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
