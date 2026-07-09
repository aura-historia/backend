use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use crate::dynamodb::product_event_record::lifecycle::ProductLifecycleEventRecord;
use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use crate::opensearch::product_image_document::ProductImageDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use common::event_id::EventId;
use common::language::record::LanguageRecord;
use common::mergeable::Mergeable;
use common::product_lifecycle::document::ProductLifecycleDocument;
use indexmap::IndexSet;
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
    pub lifecycle: Option<ProductLifecycleDocument>,

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
    pub images: Option<IndexSet<ProductImageDocument>>,

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
            lifecycle: None,
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
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
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl Mergeable for ProductUpdateDocument {
    fn merge(&mut self, other: Self) {
        let Self {
            event_id,
            price_eur,
            price_usd,
            price_gbp,
            price_aud,
            price_cad,
            price_nzd,
            price_cny,
            price_brl,
            price_pln,
            price_try,
            price_jpy,
            price_czk,
            price_rub,
            price_aed,
            price_sar,
            price_hkd,
            price_sgd,
            price_chf,
            state,
            lifecycle,
            title_de,
            title_en,
            title_fr,
            title_es,
            title_it,
            images,
            price_estimate_min_eur,
            price_estimate_min_usd,
            price_estimate_min_gbp,
            price_estimate_min_aud,
            price_estimate_min_cad,
            price_estimate_min_nzd,
            price_estimate_min_cny,
            price_estimate_min_brl,
            price_estimate_min_pln,
            price_estimate_min_try,
            price_estimate_min_jpy,
            price_estimate_min_czk,
            price_estimate_min_rub,
            price_estimate_min_aed,
            price_estimate_min_sar,
            price_estimate_min_hkd,
            price_estimate_min_sgd,
            price_estimate_min_chf,
            price_estimate_max_eur,
            price_estimate_max_usd,
            price_estimate_max_gbp,
            price_estimate_max_aud,
            price_estimate_max_cad,
            price_estimate_max_nzd,
            price_estimate_max_cny,
            price_estimate_max_brl,
            price_estimate_max_pln,
            price_estimate_max_try,
            price_estimate_max_jpy,
            price_estimate_max_czk,
            price_estimate_max_rub,
            price_estimate_max_aed,
            price_estimate_max_sar,
            price_estimate_max_hkd,
            price_estimate_max_sgd,
            price_estimate_max_chf,
            url,
            auction_start,
            auction_end,
            embedding,
            updated,
        } = other;

        self.updated = updated;
        if let Some(event_id) = event_id {
            self.event_id = Some(event_id);
        }
        if let Some(price_eur) = price_eur {
            self.price_eur = Some(price_eur);
        }
        if let Some(price_usd) = price_usd {
            self.price_usd = Some(price_usd);
        }
        if let Some(price_gbp) = price_gbp {
            self.price_gbp = Some(price_gbp);
        }
        if let Some(price_aud) = price_aud {
            self.price_aud = Some(price_aud);
        }
        if let Some(price_cad) = price_cad {
            self.price_cad = Some(price_cad);
        }
        if let Some(price_nzd) = price_nzd {
            self.price_nzd = Some(price_nzd);
        }
        if let Some(price_cny) = price_cny {
            self.price_cny = Some(price_cny);
        }
        if let Some(price_brl) = price_brl {
            self.price_brl = Some(price_brl);
        }
        if let Some(price_pln) = price_pln {
            self.price_pln = Some(price_pln);
        }
        if let Some(price_try) = price_try {
            self.price_try = Some(price_try);
        }
        if let Some(price_jpy) = price_jpy {
            self.price_jpy = Some(price_jpy);
        }
        if let Some(price_czk) = price_czk {
            self.price_czk = Some(price_czk);
        }
        if let Some(price_rub) = price_rub {
            self.price_rub = Some(price_rub);
        }
        if let Some(price_aed) = price_aed {
            self.price_aed = Some(price_aed);
        }
        if let Some(price_sar) = price_sar {
            self.price_sar = Some(price_sar);
        }
        if let Some(price_hkd) = price_hkd {
            self.price_hkd = Some(price_hkd);
        }
        if let Some(price_sgd) = price_sgd {
            self.price_sgd = Some(price_sgd);
        }
        if let Some(price_chf) = price_chf {
            self.price_chf = Some(price_chf);
        }
        if let Some(state) = state {
            self.state = Some(state);
        }
        if let Some(lifecycle) = lifecycle {
            self.lifecycle = Some(lifecycle);
        }
        if let Some(title_de) = title_de {
            self.title_de = Some(title_de);
        }
        if let Some(title_en) = title_en {
            self.title_en = Some(title_en);
        }
        if let Some(title_fr) = title_fr {
            self.title_fr = Some(title_fr);
        }
        if let Some(title_es) = title_es {
            self.title_es = Some(title_es);
        }
        if let Some(title_it) = title_it {
            self.title_it = Some(title_it);
        }
        if let Some(images) = images {
            self.images = Some(images);
        }
        if let Some(price_estimate_min_eur) = price_estimate_min_eur {
            self.price_estimate_min_eur = Some(price_estimate_min_eur);
        }
        if let Some(price_estimate_min_usd) = price_estimate_min_usd {
            self.price_estimate_min_usd = Some(price_estimate_min_usd);
        }
        if let Some(price_estimate_min_gbp) = price_estimate_min_gbp {
            self.price_estimate_min_gbp = Some(price_estimate_min_gbp);
        }
        if let Some(price_estimate_min_aud) = price_estimate_min_aud {
            self.price_estimate_min_aud = Some(price_estimate_min_aud);
        }
        if let Some(price_estimate_min_cad) = price_estimate_min_cad {
            self.price_estimate_min_cad = Some(price_estimate_min_cad);
        }
        if let Some(price_estimate_min_nzd) = price_estimate_min_nzd {
            self.price_estimate_min_nzd = Some(price_estimate_min_nzd);
        }
        if let Some(price_estimate_min_cny) = price_estimate_min_cny {
            self.price_estimate_min_cny = Some(price_estimate_min_cny);
        }
        if let Some(price_estimate_min_brl) = price_estimate_min_brl {
            self.price_estimate_min_brl = Some(price_estimate_min_brl);
        }
        if let Some(price_estimate_min_pln) = price_estimate_min_pln {
            self.price_estimate_min_pln = Some(price_estimate_min_pln);
        }
        if let Some(price_estimate_min_try) = price_estimate_min_try {
            self.price_estimate_min_try = Some(price_estimate_min_try);
        }
        if let Some(price_estimate_min_jpy) = price_estimate_min_jpy {
            self.price_estimate_min_jpy = Some(price_estimate_min_jpy);
        }
        if let Some(price_estimate_min_czk) = price_estimate_min_czk {
            self.price_estimate_min_czk = Some(price_estimate_min_czk);
        }
        if let Some(price_estimate_min_rub) = price_estimate_min_rub {
            self.price_estimate_min_rub = Some(price_estimate_min_rub);
        }
        if let Some(price_estimate_min_aed) = price_estimate_min_aed {
            self.price_estimate_min_aed = Some(price_estimate_min_aed);
        }
        if let Some(price_estimate_min_sar) = price_estimate_min_sar {
            self.price_estimate_min_sar = Some(price_estimate_min_sar);
        }
        if let Some(price_estimate_min_hkd) = price_estimate_min_hkd {
            self.price_estimate_min_hkd = Some(price_estimate_min_hkd);
        }
        if let Some(price_estimate_min_sgd) = price_estimate_min_sgd {
            self.price_estimate_min_sgd = Some(price_estimate_min_sgd);
        }
        if let Some(price_estimate_min_chf) = price_estimate_min_chf {
            self.price_estimate_min_chf = Some(price_estimate_min_chf);
        }
        if let Some(price_estimate_max_eur) = price_estimate_max_eur {
            self.price_estimate_max_eur = Some(price_estimate_max_eur);
        }
        if let Some(price_estimate_max_usd) = price_estimate_max_usd {
            self.price_estimate_max_usd = Some(price_estimate_max_usd);
        }
        if let Some(price_estimate_max_gbp) = price_estimate_max_gbp {
            self.price_estimate_max_gbp = Some(price_estimate_max_gbp);
        }
        if let Some(price_estimate_max_aud) = price_estimate_max_aud {
            self.price_estimate_max_aud = Some(price_estimate_max_aud);
        }
        if let Some(price_estimate_max_cad) = price_estimate_max_cad {
            self.price_estimate_max_cad = Some(price_estimate_max_cad);
        }
        if let Some(price_estimate_max_nzd) = price_estimate_max_nzd {
            self.price_estimate_max_nzd = Some(price_estimate_max_nzd);
        }
        if let Some(price_estimate_max_cny) = price_estimate_max_cny {
            self.price_estimate_max_cny = Some(price_estimate_max_cny);
        }
        if let Some(price_estimate_max_brl) = price_estimate_max_brl {
            self.price_estimate_max_brl = Some(price_estimate_max_brl);
        }
        if let Some(price_estimate_max_pln) = price_estimate_max_pln {
            self.price_estimate_max_pln = Some(price_estimate_max_pln);
        }
        if let Some(price_estimate_max_try) = price_estimate_max_try {
            self.price_estimate_max_try = Some(price_estimate_max_try);
        }
        if let Some(price_estimate_max_jpy) = price_estimate_max_jpy {
            self.price_estimate_max_jpy = Some(price_estimate_max_jpy);
        }
        if let Some(price_estimate_max_czk) = price_estimate_max_czk {
            self.price_estimate_max_czk = Some(price_estimate_max_czk);
        }
        if let Some(price_estimate_max_rub) = price_estimate_max_rub {
            self.price_estimate_max_rub = Some(price_estimate_max_rub);
        }
        if let Some(price_estimate_max_aed) = price_estimate_max_aed {
            self.price_estimate_max_aed = Some(price_estimate_max_aed);
        }
        if let Some(price_estimate_max_sar) = price_estimate_max_sar {
            self.price_estimate_max_sar = Some(price_estimate_max_sar);
        }
        if let Some(price_estimate_max_hkd) = price_estimate_max_hkd {
            self.price_estimate_max_hkd = Some(price_estimate_max_hkd);
        }
        if let Some(price_estimate_max_sgd) = price_estimate_max_sgd {
            self.price_estimate_max_sgd = Some(price_estimate_max_sgd);
        }
        if let Some(price_estimate_max_chf) = price_estimate_max_chf {
            self.price_estimate_max_chf = Some(price_estimate_max_chf);
        }
        if let Some(url) = url {
            self.url = Some(url);
        }
        if let Some(auction_start) = auction_start {
            self.auction_start = Some(auction_start);
        }
        if let Some(auction_end) = auction_end {
            self.auction_end = Some(auction_end);
        }
        if let Some(embedding) = embedding {
            self.embedding = Some(embedding);
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
            lifecycle: None,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            title_fr: event_record.title_fr,
            title_es: event_record.title_es,
            title_it: event_record.title_it,
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
            embedding: None,
            updated: event_record.timestamp,
        }
    }
}

impl From<ProductLifecycleEventRecord> for ProductUpdateDocument {
    fn from(event_record: ProductLifecycleEventRecord) -> Self {
        ProductUpdateDocument {
            event_id: Some(event_record.event_id),
            lifecycle: Some(event_record.new_lifecycle.into()),
            updated: event_record.timestamp,
            ..ProductUpdateDocument::default()
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
            lifecycle: None,
            embedding: event_record.embedding,
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
            _ => {}
        }
        update
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::title::Title;
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
                images: Some(
                    config
                        .fake_with_rng::<Vec<ProductImageDocument>, _>(rng)
                        .into_iter()
                        .collect(),
                ),
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
                lifecycle: config.fake_with_rng(rng),
                embedding: None,
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
    use common::mergeable::Mergeable;
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

    #[test]
    fn should_merge_product_update_document() {
        let mut current = ProductUpdateDocument {
            title_en: Some("Chair".to_string()),
            ..ProductUpdateDocument::default()
        };
        let other = ProductUpdateDocument {
            title_fr: Some("Chaise".to_string()),
            embedding: Some(vec![0.1, 0.2]),
            ..ProductUpdateDocument::default()
        };

        current.merge(other);

        assert_eq!(Some("Chair".to_string()), current.title_en);
        assert_eq!(Some("Chaise".to_string()), current.title_fr);
        assert_eq!(Some(vec![0.1, 0.2]), current.embedding);
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
}
