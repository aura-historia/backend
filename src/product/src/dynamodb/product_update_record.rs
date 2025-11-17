use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_state_record::ProductStateRecord;
use common::dynamodb_update::DynamoDbUpdate;
use common::event_id::EventId;
use common::price::record::PriceRecord;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProductRecordUpdate {
    pub event_id: EventId,

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
    pub state: Option<ProductStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for ProductRecordUpdate {}

impl Default for ProductRecordUpdate {
    fn default() -> Self {
        Self {
            event_id: EventId::new(),
            price_native: None,
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            title_de: None,
            title_en: None,
            description_de: None,
            description_en: None,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl From<ProductEventRecord> for ProductRecordUpdate {
    fn from(event: ProductEventRecord) -> Self {
        ProductRecordUpdate {
            event_id: event.event_id,
            price_native: event.new_price_native,
            price_eur: event.new_price_eur,
            price_usd: event.new_price_usd,
            price_gbp: event.new_price_gbp,
            price_aud: event.new_price_aud,
            price_cad: event.new_price_cad,
            price_nzd: event.new_price_nzd,
            state: event.new_state,
            title_de: event.title_de,
            title_en: event.title_en,
            description_de: event.description_de,
            description_en: event.description_en,
            updated: event.timestamp,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::core::{description::Description, title::Title};

    use super::*;
    use common::price::domain::{MonetaryAmount, Price};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductRecordUpdate {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let price_native: Option<PriceRecord> =
                Some(config.fake_with_rng::<Price, _>(rng).into());
            let state: ProductStateRecord = config.fake_with_rng(rng);

            ProductRecordUpdate {
                event_id: config.fake_with_rng(rng),
                price_native,
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state: Some(state),
                title_de: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).into()),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).into()),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_update_record::ProductRecordUpdate;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_record_update() {
            let _ = Faker.fake::<ProductRecordUpdate>();
        }
    }
}
