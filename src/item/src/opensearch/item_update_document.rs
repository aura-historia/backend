use crate::dynamodb::item_event_record::ItemEventRecord;
use crate::opensearch::item_state_document::ItemStateDocument;
use common::event_id::EventId;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemUpdateDocument {
    pub event_id: EventId,

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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ItemStateDocument>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl Default for ItemUpdateDocument {
    fn default() -> Self {
        Self {
            event_id: EventId::new(),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            embedding: None,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl From<ItemEventRecord> for ItemUpdateDocument {
    fn from(event_record: ItemEventRecord) -> Self {
        let state = event_record.new_state.map(ItemStateDocument::from);
        ItemUpdateDocument {
            event_id: event_record.event_id,
            price_eur: event_record.new_price_eur,
            price_usd: event_record.new_price_usd,
            price_gbp: event_record.new_price_gbp,
            price_aud: event_record.new_price_aud,
            price_cad: event_record.new_price_cad,
            price_nzd: event_record.new_price_nzd,
            state,
            embedding: None,
            updated: event_record.timestamp,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ItemUpdateDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let state = config.fake_with_rng(rng);
            ItemUpdateDocument {
                event_id: config.fake_with_rng(rng),
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state,
                embedding: None,
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::item_update_document::ItemUpdateDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item_update_document() {
            let _ = Faker.fake::<ItemUpdateDocument>();
        }
    }
}
