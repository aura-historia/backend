use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::opensearch::product_state_document::ProductStateDocument;
use common::event_id::EventId;
use serde::Serialize;
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ProductUpdateDocument {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_id: Option<EventId>,

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
    pub state: Option<ProductStateDocument>,

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
    pub text_embedding: Option<Vec<f32>>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl Default for ProductUpdateDocument {
    fn default() -> Self {
        Self {
            event_id: Some(EventId::new()),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            description_de: None,
            description_en: None,
            description_fr: None,
            description_es: None,
            text_embedding: None,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl From<ProductEventRecord> for ProductUpdateDocument {
    fn from(event_record: ProductEventRecord) -> Self {
        let state = event_record.new_state.map(ProductStateDocument::from);
        ProductUpdateDocument {
            event_id: Some(event_record.event_id),
            price_eur: event_record.new_price_eur,
            price_usd: event_record.new_price_usd,
            price_gbp: event_record.new_price_gbp,
            price_aud: event_record.new_price_aud,
            price_cad: event_record.new_price_cad,
            price_nzd: event_record.new_price_nzd,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            title_fr: event_record.title_fr,
            title_es: event_record.title_es,
            description_de: event_record.description_de,
            description_en: event_record.description_en,
            description_fr: event_record.description_fr,
            description_es: event_record.description_es,
            state,
            text_embedding: None,
            updated: event_record.timestamp,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::{description::Description, title::Title};
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductUpdateDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let state = config.fake_with_rng(rng);
            ProductUpdateDocument {
                event_id: config.fake_with_rng(rng),
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                title_de: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).into()),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_fr: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_es: Some(config.fake_with_rng::<Description, _>(rng).into()),
                state,
                text_embedding: None,
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::product_update_document::ProductUpdateDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_update_document() {
            let _ = Faker.fake::<ProductUpdateDocument>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::opensearch::{
        product_document::ProductDocument, product_update_document::ProductUpdateDocument,
    };

    #[test]
    fn should_be_subset_of_product_document() {
        assert!(
            ProductUpdateDocument::SERDE_FIELDS
                .iter()
                .all(|field| ProductDocument::SERDE_FIELDS.contains(field))
        )
    }
}
