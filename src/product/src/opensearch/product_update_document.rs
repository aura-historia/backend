use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
use crate::opensearch::product_image_document::ProductImageDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::provenance_document::ProvenanceDocument;
use crate::opensearch::restoration_document::RestorationDocument;
use common::category_key::CategoryId;
use common::event_id::EventId;
use common::language::record::LanguageRecord;
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
            price_cny: event_record.new_price_cny,
            price_brl: event_record.new_price_brl,
            price_pln: event_record.new_price_pln,
            price_try: event_record.new_price_try,
            price_jpy: event_record.new_price_jpy,
            price_czk: event_record.new_price_czk,
            price_rub: event_record.new_price_rub,
            price_aed: event_record.new_price_aed,
            price_sar: event_record.new_price_sar,
            price_hkd: event_record.new_price_hkd,
            price_sgd: event_record.new_price_sgd,
            price_chf: event_record.new_price_chf,
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
            price_estimate_min_cny: event_record.new_price_estimate_min_cny,
            price_estimate_min_brl: event_record.new_price_estimate_min_brl,
            price_estimate_min_pln: event_record.new_price_estimate_min_pln,
            price_estimate_min_try: event_record.new_price_estimate_min_try,
            price_estimate_min_jpy: event_record.new_price_estimate_min_jpy,
            price_estimate_min_czk: event_record.new_price_estimate_min_czk,
            price_estimate_min_rub: event_record.new_price_estimate_min_rub,
            price_estimate_min_aed: event_record.new_price_estimate_min_aed,
            price_estimate_min_sar: event_record.new_price_estimate_min_sar,
            price_estimate_min_hkd: event_record.new_price_estimate_min_hkd,
            price_estimate_min_sgd: event_record.new_price_estimate_min_sgd,
            price_estimate_min_chf: event_record.new_price_estimate_min_chf,
            price_estimate_max_eur: event_record.new_price_estimate_max_eur,
            price_estimate_max_usd: event_record.new_price_estimate_max_usd,
            price_estimate_max_gbp: event_record.new_price_estimate_max_gbp,
            price_estimate_max_aud: event_record.new_price_estimate_max_aud,
            price_estimate_max_cad: event_record.new_price_estimate_max_cad,
            price_estimate_max_nzd: event_record.new_price_estimate_max_nzd,
            price_estimate_max_cny: event_record.new_price_estimate_max_cny,
            price_estimate_max_brl: event_record.new_price_estimate_max_brl,
            price_estimate_max_pln: event_record.new_price_estimate_max_pln,
            price_estimate_max_try: event_record.new_price_estimate_max_try,
            price_estimate_max_jpy: event_record.new_price_estimate_max_jpy,
            price_estimate_max_czk: event_record.new_price_estimate_max_czk,
            price_estimate_max_rub: event_record.new_price_estimate_max_rub,
            price_estimate_max_aed: event_record.new_price_estimate_max_aed,
            price_estimate_max_sar: event_record.new_price_estimate_max_sar,
            price_estimate_max_hkd: event_record.new_price_estimate_max_hkd,
            price_estimate_max_sgd: event_record.new_price_estimate_max_sgd,
            price_estimate_max_chf: event_record.new_price_estimate_max_chf,
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
        let mut update = ProductUpdateDocument {
            event_id: Some(event_record.event_id),
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
        };
        match (
            event_record.event_type,
            event_record.target_language,
            event_record.target,
        ) {
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
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
                Some(LanguageRecord::De),
                Some(target),
            ) => update.description_de = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
                Some(LanguageRecord::En),
                Some(target),
            ) => update.description_en = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
                Some(LanguageRecord::Fr),
                Some(target),
            ) => update.description_fr = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
                Some(LanguageRecord::Es),
                Some(target),
            ) => update.description_es = Some(target),
            (
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
                Some(LanguageRecord::It),
                Some(target),
            ) => update.description_it = Some(target),
            _ => {}
        }
        update
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
    use crate::dynamodb::product_event_record::enrichment::{
        ProductEnrichmentEventRecord, mk_pk, mk_sk,
    };
    use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
    use crate::opensearch::{
        product_document::ProductDocument, product_update_document::ProductUpdateDocument,
    };
    use common::event_id::EventId;
    use common::language::record::LanguageRecord;
    use common::product_id::ProductId;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use time::OffsetDateTime;

    #[test]
    fn should_be_subset_of_product_document() {
        assert!(
            ProductUpdateDocument::SERDE_FIELDS
                .iter()
                .all(|field| ProductDocument::SERDE_FIELDS.contains(field))
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
            category_id: None,
            period_id: None,
            source_language: Some(LanguageRecord::En),
            target_language: Some(target_language),
            target: Some(target.to_string()),
            embedding: None,
            native_title: None,
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            timestamp: OffsetDateTime::now_utc(),
        }
    }

    #[rstest::rstest]
    #[case(LanguageRecord::De, "titleDe")]
    #[case(LanguageRecord::En, "titleEn")]
    #[case(LanguageRecord::Fr, "titleFr")]
    #[case(LanguageRecord::Es, "titleEs")]
    #[case(LanguageRecord::It, "titleIt")]
    fn should_set_title_field_when_translated_title_enrichment_event_for_supported_language(
        #[case] language: LanguageRecord,
        #[case] expected_field: &str,
    ) {
        let record = make_translation_record(
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
            language,
            "translated title",
        );
        let update = ProductUpdateDocument::from(record);
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(
            json[expected_field].as_str(),
            Some("translated title"),
            "Expected field '{expected_field}' to contain the translated title"
        );
        let title_fields = ["titleDe", "titleEn", "titleFr", "titleEs", "titleIt"];
        for field in title_fields.iter().filter(|&&f| f != expected_field) {
            assert!(
                json.get(field).is_none(),
                "Expected field '{field}' to be absent but it was present"
            );
        }
    }

    #[rstest::rstest]
    #[case(LanguageRecord::De, "descriptionDe")]
    #[case(LanguageRecord::En, "descriptionEn")]
    #[case(LanguageRecord::Fr, "descriptionFr")]
    #[case(LanguageRecord::Es, "descriptionEs")]
    #[case(LanguageRecord::It, "descriptionIt")]
    fn should_set_description_field_when_translated_description_enrichment_event_for_supported_language(
        #[case] language: LanguageRecord,
        #[case] expected_field: &str,
    ) {
        let record = make_translation_record(
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
            language,
            "translated description",
        );
        let update = ProductUpdateDocument::from(record);
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(
            json[expected_field].as_str(),
            Some("translated description"),
            "Expected field '{expected_field}' to contain the translated description"
        );
        let description_fields = [
            "descriptionDe",
            "descriptionEn",
            "descriptionFr",
            "descriptionEs",
            "descriptionIt",
        ];
        for field in description_fields.iter().filter(|&&f| f != expected_field) {
            assert!(
                json.get(field).is_none(),
                "Expected field '{field}' to be absent but it was present"
            );
        }
    }
}
