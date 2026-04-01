use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
use crate::opensearch::product_image_document::ProductImageDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::provenance_document::ProvenanceDocument;
use crate::opensearch::restoration_document::RestorationDocument;
use common::category_key::CategoryId;
use common::event_id::EventId;
use common::period_key::PeriodId;
use common::year::Year;
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
    pub images: Option<Vec<ProductImageDocument>>,

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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<url::Url>,

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
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
            description_de: None,
            description_en: None,
            description_fr: None,
            description_es: None,
            description_it: None,
            images: None,
            price_estimate_min_eur: None,
            price_estimate_min_usd: None,
            price_estimate_min_gbp: None,
            price_estimate_min_aud: None,
            price_estimate_min_cad: None,
            price_estimate_min_nzd: None,
            price_estimate_max_eur: None,
            price_estimate_max_usd: None,
            price_estimate_max_gbp: None,
            price_estimate_max_aud: None,
            price_estimate_max_cad: None,
            price_estimate_max_nzd: None,
            url: None,
            auction_start: None,
            auction_end: None,
            embedding: None,
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl From<ProductDomainEventRecord> for ProductUpdateDocument {
    fn from(event_record: ProductDomainEventRecord) -> Self {
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
            title_it: event_record.title_it,
            description_de: event_record.description_de,
            description_en: event_record.description_en,
            description_fr: event_record.description_fr,
            description_es: event_record.description_es,
            description_it: event_record.description_it,
            images: event_record
                .images
                .map(|images| images.into_iter().map(ProductImageDocument::from).collect()),
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
            url: event_record.url,
            auction_start: event_record.auction_start,
            auction_end: event_record.auction_end,
            state,
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
            embedding: None,
            origin_year_min: event_record.origin_year_min,
            origin_year: event_record.origin_year,
            origin_year_max: event_record.origin_year_max,
            authenticity: event_record.authenticity.map(Into::into),
            condition: event_record.condition.map(Into::into),
            provenance: event_record.provenance.map(Into::into),
            restoration: event_record.restoration.map(Into::into),
            updated: event_record.timestamp,
        }
    }
}

impl From<ProductEnrichmentEventRecord> for ProductUpdateDocument {
    fn from(event_record: ProductEnrichmentEventRecord) -> Self {
        ProductUpdateDocument {
            event_id: Some(event_record.event_id),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
            description_de: None,
            description_en: None,
            description_fr: None,
            description_es: None,
            description_it: None,
            images: None,
            price_estimate_min_eur: None,
            price_estimate_min_usd: None,
            price_estimate_min_gbp: None,
            price_estimate_min_aud: None,
            price_estimate_min_cad: None,
            price_estimate_min_nzd: None,
            price_estimate_max_eur: None,
            price_estimate_max_usd: None,
            price_estimate_max_gbp: None,
            price_estimate_max_aud: None,
            price_estimate_max_cad: None,
            price_estimate_max_nzd: None,
            url: None,
            auction_start: None,
            auction_end: None,
            state: None,
            category_id: event_record.category_id,
            period_id: event_record.period_id,
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
            embedding: event_record.embedding,
            origin_year_min: event_record.origin_year_min,
            origin_year: event_record.origin_year,
            origin_year_max: event_record.origin_year_max,
            authenticity: event_record.authenticity.map(Into::into),
            condition: event_record.condition.map(Into::into),
            provenance: event_record.provenance.map(Into::into),
            restoration: event_record.restoration.map(Into::into),
            updated: event_record.timestamp,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::{description::Description, title::Title};
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductUpdateDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
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
                title_it: Some(config.fake_with_rng::<Title, _>(rng).into()),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_fr: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_es: Some(config.fake_with_rng::<Description, _>(rng).into()),
                description_it: Some(config.fake_with_rng::<Description, _>(rng).into()),
                images: Some(config.fake_with_rng(rng)),
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
                url: Some(
                    url::Url::parse(&format!(
                        "https://foo.bar/item/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ),
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
                state,
                category_id: Some(config.fake_with_rng(rng)),
                period_id: Some(config.fake_with_rng(rng)),
                category_name_de: Some(config.fake_with_rng(rng)),
                category_name_en: Some(config.fake_with_rng(rng)),
                category_name_fr: Some(config.fake_with_rng(rng)),
                category_name_es: Some(config.fake_with_rng(rng)),
                category_name_it: Some(config.fake_with_rng(rng)),
                period_name_de: Some(config.fake_with_rng(rng)),
                period_name_en: Some(config.fake_with_rng(rng)),
                period_name_fr: Some(config.fake_with_rng(rng)),
                period_name_es: Some(config.fake_with_rng(rng)),
                period_name_it: Some(config.fake_with_rng(rng)),
                embedding: None,
                origin_year_min: config.fake_with_rng(rng),
                origin_year: config.fake_with_rng(rng),
                origin_year_max: config.fake_with_rng(rng),
                authenticity: config.fake_with_rng(rng),
                condition: config.fake_with_rng(rng),
                provenance: config.fake_with_rng(rng),
                restoration: config.fake_with_rng(rng),
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
