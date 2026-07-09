use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use crate::dynamodb::product_event_record::lifecycle::ProductLifecycleEventRecord;
use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use crate::dynamodb::product_image_record::ProductImageRecord;
use crate::dynamodb::product_state_record::ProductStateRecord;
use common::dynamodb_update::DynamoDbUpdate;
use common::event_id::EventId;
use common::language::record::LanguageRecord;
use common::price::record::PriceRecord;
use common::product_lifecycle::record::ProductLifecycleRecord;
use indexmap::IndexSet;
use serde::Serialize;
use serde_fields::SerdeField;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, SerdeField)]
pub struct ProductRecordUpdate {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_id: Option<EventId>,

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
    pub state: Option<ProductStateRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lifecycle: Option<ProductLifecycleRecord>,

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
    pub images: Option<IndexSet<ProductImageRecord>>,

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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_url: Option<Url>,

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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for ProductRecordUpdate {}

impl Default for ProductRecordUpdate {
    fn default() -> Self {
        Self {
            event_id: Some(EventId::new()),
            price_native: None,
            price_eur: None,
            price_usd: None,
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
            state: None,
            lifecycle: None,
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
            images: None,
            price_estimate_min_native: None,
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
            price_estimate_max_native: None,
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
            url: None,
            view_url: None,
            auction_start: None,
            auction_end: None,
            embedding: None,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl From<ProductDomainEventRecord> for ProductRecordUpdate {
    fn from(event: ProductDomainEventRecord) -> Self {
        ProductRecordUpdate {
            event_id: Some(event.event_id),
            price_native: event.new_price_native,
            price_eur: event.new_price_eur,
            price_usd: event.new_price_usd,
            price_gbp: event.new_price_gbp,
            price_aud: event.new_price_aud,
            price_cad: event.new_price_cad,
            price_nzd: event.new_price_nzd,
            price_cny: event.new_price_cny,
            price_brl: event.new_price_brl,
            price_pln: event.new_price_pln,
            price_try: event.new_price_try,
            price_jpy: event.new_price_jpy,
            price_czk: event.new_price_czk,
            price_rub: event.new_price_rub,
            price_aed: event.new_price_aed,
            price_sar: event.new_price_sar,
            price_hkd: event.new_price_hkd,
            price_sgd: event.new_price_sgd,
            price_chf: event.new_price_chf,
            state: event.new_state,
            lifecycle: None,
            title_de: event.title_de,
            title_en: event.title_en,
            title_fr: event.title_fr,
            title_es: event.title_es,
            title_it: event.title_it,
            images: event.images,
            price_estimate_min_native: event.new_price_estimate_min_native,
            price_estimate_min_eur: event.new_price_estimate_min_eur,
            price_estimate_min_usd: event.new_price_estimate_min_usd,
            price_estimate_min_gbp: event.new_price_estimate_min_gbp,
            price_estimate_min_aud: event.new_price_estimate_min_aud,
            price_estimate_min_cad: event.new_price_estimate_min_cad,
            price_estimate_min_nzd: event.new_price_estimate_min_nzd,
            price_estimate_min_cny: event.new_price_estimate_min_cny,
            price_estimate_min_brl: event.new_price_estimate_min_brl,
            price_estimate_min_pln: event.new_price_estimate_min_pln,
            price_estimate_min_try: event.new_price_estimate_min_try,
            price_estimate_min_jpy: event.new_price_estimate_min_jpy,
            price_estimate_min_czk: event.new_price_estimate_min_czk,
            price_estimate_min_rub: event.new_price_estimate_min_rub,
            price_estimate_min_aed: event.new_price_estimate_min_aed,
            price_estimate_min_sar: event.new_price_estimate_min_sar,
            price_estimate_min_hkd: event.new_price_estimate_min_hkd,
            price_estimate_min_sgd: event.new_price_estimate_min_sgd,
            price_estimate_min_chf: event.new_price_estimate_min_chf,
            price_estimate_max_native: event.new_price_estimate_max_native,
            price_estimate_max_eur: event.new_price_estimate_max_eur,
            price_estimate_max_usd: event.new_price_estimate_max_usd,
            price_estimate_max_gbp: event.new_price_estimate_max_gbp,
            price_estimate_max_aud: event.new_price_estimate_max_aud,
            price_estimate_max_cad: event.new_price_estimate_max_cad,
            price_estimate_max_nzd: event.new_price_estimate_max_nzd,
            price_estimate_max_cny: event.new_price_estimate_max_cny,
            price_estimate_max_brl: event.new_price_estimate_max_brl,
            price_estimate_max_pln: event.new_price_estimate_max_pln,
            price_estimate_max_try: event.new_price_estimate_max_try,
            price_estimate_max_jpy: event.new_price_estimate_max_jpy,
            price_estimate_max_czk: event.new_price_estimate_max_czk,
            price_estimate_max_rub: event.new_price_estimate_max_rub,
            price_estimate_max_aed: event.new_price_estimate_max_aed,
            price_estimate_max_sar: event.new_price_estimate_max_sar,
            price_estimate_max_hkd: event.new_price_estimate_max_hkd,
            price_estimate_max_sgd: event.new_price_estimate_max_sgd,
            price_estimate_max_chf: event.new_price_estimate_max_chf,
            url: event.url.clone(),
            view_url: event.view_url,
            auction_start: event.auction_start,
            auction_end: event.auction_end,
            embedding: None,
            updated: event.timestamp,
        }
    }
}

impl From<ProductLifecycleEventRecord> for ProductRecordUpdate {
    fn from(event: ProductLifecycleEventRecord) -> Self {
        ProductRecordUpdate {
            event_id: Some(event.event_id),
            lifecycle: Some(event.new_lifecycle),
            updated: event.timestamp,
            ..ProductRecordUpdate::default()
        }
    }
}

impl From<ProductEnrichmentEventRecord> for ProductRecordUpdate {
    fn from(event: ProductEnrichmentEventRecord) -> Self {
        let mut update = ProductRecordUpdate {
            event_id: Some(event.event_id),
            price_native: None,
            price_eur: None,
            price_usd: None,
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
            state: None,
            lifecycle: None,
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
            images: None,
            price_estimate_min_native: None,
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
            price_estimate_max_native: None,
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
            url: None,
            view_url: None,
            auction_start: None,
            auction_end: None,
            embedding: event.embedding,
            updated: event.timestamp,
        };
        match (event.event_type, event.target_language, event.target) {
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                Some(LanguageRecord::De),
                Some(target),
            ) => update.title_de = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                Some(LanguageRecord::En),
                Some(target),
            ) => update.title_en = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                Some(LanguageRecord::Fr),
                Some(target),
            ) => update.title_fr = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                Some(LanguageRecord::Es),
                Some(target),
            ) => update.title_es = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                Some(LanguageRecord::It),
                Some(target),
            ) => update.title_it = Some(target),
            _ => {}
        }
        update
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::title::Title;
    use common::price::domain::{MonetaryAmount, Price};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductRecordUpdate {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
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
                price_cny: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_brl: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_pln: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_try: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_jpy: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_czk: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_rub: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aed: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_sar: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_hkd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_sgd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_chf: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state: Some(state),
                title_de: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).into()),
                title_it: Some(config.fake_with_rng::<Title, _>(rng).into()),
                images: Some(
                    config
                        .fake_with_rng::<Vec<ProductImageRecord>, _>(rng)
                        .into_iter()
                        .collect(),
                ),
                price_estimate_min_native: Some(config.fake_with_rng::<Price, _>(rng).into()),
                price_estimate_min_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_cny: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_brl: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_pln: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_try: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_jpy: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_czk: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_rub: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_aed: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_sar: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_hkd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_sgd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_chf: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_native: Some(config.fake_with_rng::<Price, _>(rng).into()),
                price_estimate_max_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_cny: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_brl: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_pln: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_try: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_jpy: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_czk: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_rub: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_aed: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_sar: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_hkd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_sgd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_chf: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                lifecycle: config.fake_with_rng(rng),
                url: Some(
                    url::Url::parse(&format!(
                        "https://foo.bar/item/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ),
                view_url: None,
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
                embedding: if config.fake_with_rng(rng) {
                    Some(fake::vec![f32; 768])
                } else {
                    None
                },
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

#[cfg(test)]
mod tests {
    use crate::dynamodb::product_event_record::enrichment::{
        ProductEnrichmentEventRecord, mk_pk, mk_sk,
    };
    use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
    use crate::dynamodb::{
        product_record::ProductRecord, product_update_record::ProductRecordUpdate,
    };
    use common::event_id::EventId;
    use common::language::record::LanguageRecord;
    use common::product_id::ProductId;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use time::OffsetDateTime;

    #[test]
    fn should_be_subset_of_product_record() {
        assert!(
            ProductRecordUpdate::SERDE_FIELDS
                .iter()
                .all(|field| ProductRecord::SERDE_FIELDS.contains(field))
        )
    }

    fn make_translation_record(
        event_type: ProductEnrichmentEventTypeRecord,
        target_language: LanguageRecord,
        target: &str,
    ) -> ProductEnrichmentEventRecord {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let event_id = EventId::new();
        ProductEnrichmentEventRecord {
            pk: mk_pk(&shop_id, &shops_product_id),
            sk: mk_sk(&event_id),
            product_id: ProductId::new(),
            event_id,
            event_type,
            event_type_schema_version: 0,
            shop_id,
            seller_id: ShopId::new(),
            shops_product_id,
            source_language: Some(LanguageRecord::En),
            target_language: Some(target_language),
            target: Some(target.to_string()),
            embedding: None,
            native_title: None,
            native_title_language: None,
            timestamp: OffsetDateTime::now_utc(),
        }
    }

    #[rstest::rstest]
    #[case(LanguageRecord::De, "title_de")]
    #[case(LanguageRecord::En, "title_en")]
    #[case(LanguageRecord::Fr, "title_fr")]
    #[case(LanguageRecord::Es, "title_es")]
    #[case(LanguageRecord::It, "title_it")]
    fn should_set_title_field_when_translated_title_enrichment_event_for_supported_language(
        #[case] language: LanguageRecord,
        #[case] expected_field: &str,
    ) {
        let record = make_translation_record(
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
            language,
            "translated title",
        );
        let update = ProductRecordUpdate::from(record);
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(
            json[expected_field].as_str(),
            Some("translated title"),
            "Expected field '{expected_field}' to contain the translated title"
        );
        let title_fields = ["title_de", "title_en", "title_fr", "title_es", "title_it"];
        for field in title_fields.iter().filter(|&&f| f != expected_field) {
            assert!(
                json.get(field).is_none(),
                "Expected field '{field}' to be absent but it was present"
            );
        }
    }
}
