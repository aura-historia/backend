use common::{
    language::record::{LanguageRecord, TextRecord},
    product_id::ProductId,
    shop_id::ShopId,
    shops_product_id::ShopsProductId,
    year::Year,
};
use product::{
    dynamodb::{
        authenticity_record::AuthenticityRecord, condition_record::ConditionRecord,
        product_image_record::ProductImageRecord, product_update_record::ProductRecordUpdate,
        provenance_record::ProvenanceRecord, restoration_record::RestorationRecord,
    },
    opensearch::{
        authenticity_document::AuthenticityDocument, condition_document::ConditionDocument,
        product_image_document::ProductImageDocument,
        product_update_document::ProductUpdateDocument, provenance_document::ProvenanceDocument,
        restoration_document::RestorationDocument,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;

pub trait HasProductId {
    fn product_id(&self) -> ProductId;
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitialPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_title: TextRecord,
    pub native_description: Option<TextRecord>,
    pub images: Vec<ProductImageRecord>,
}

impl HasProductId for InitialPipeProduct {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleansedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_title: TextRecord,
    pub native_description: Option<TextRecord>,
    pub images: Vec<ProductImageRecord>,
}

impl HasProductId for CleansedPipeProduct {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub images: Vec<ProductImageRecord>,
}

impl HasProductId for TranslatedPipeProduct {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEmbeddedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub images: Vec<ProductImageRecord>,
    pub text_embedding: Vec<f32>,
}

impl HasProductId for TextEmbeddedPipeProduct {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeExtractedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub images: Vec<ProductImageRecord>,
    pub text_embedding: Vec<f32>,
    pub origin_year_min: Option<Year>,
    pub origin_year: Option<Year>,
    pub origin_year_max: Option<Year>,
    pub authenticity: AuthenticityRecord,
    pub condition: ConditionRecord,
    pub provenance: ProvenanceRecord,
    pub restoration: RestorationRecord,
}

impl HasProductId for AttributeExtractedPipeProduct {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub images: Vec<ProductImageRecord>,
    pub text_embedding: Vec<f32>,
    pub origin_year_min: Option<Year>,
    pub origin_year: Option<Year>,
    pub origin_year_max: Option<Year>,
    pub authenticity: Option<AuthenticityRecord>,
    pub condition: Option<ConditionRecord>,
    pub provenance: Option<ProvenanceRecord>,
    pub restoration: Option<RestorationRecord>,
}

impl HasProductId for CompletedPipeProduct {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

impl From<CompletedPipeProduct> for ProductRecordUpdate {
    fn from(completed_pipe_product: CompletedPipeProduct) -> Self {
        let mut titles = completed_pipe_product.other_title;
        titles.insert(
            completed_pipe_product.native_title.language,
            completed_pipe_product.native_title.text,
        );
        let mut descriptions = completed_pipe_product.other_description;
        if let Some(description) = completed_pipe_product.native_description {
            descriptions.insert(description.language, description.text);
        }

        ProductRecordUpdate {
            event_id: None,
            price_native: None,
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            title_de: titles.remove(&LanguageRecord::De),
            title_en: titles.remove(&LanguageRecord::En),
            title_fr: titles.remove(&LanguageRecord::Fr),
            title_es: titles.remove(&LanguageRecord::Es),
            description_de: descriptions.remove(&LanguageRecord::De),
            description_en: descriptions.remove(&LanguageRecord::En),
            description_fr: descriptions.remove(&LanguageRecord::Fr),
            description_es: descriptions.remove(&LanguageRecord::Es),
            images: Some(completed_pipe_product.images),
            text_embedding: Some(completed_pipe_product.text_embedding),
            origin_year_min: completed_pipe_product.origin_year_min,
            origin_year: completed_pipe_product.origin_year,
            origin_year_max: completed_pipe_product.origin_year_max,
            authenticity: completed_pipe_product.authenticity,
            condition: completed_pipe_product.condition,
            provenance: completed_pipe_product.provenance,
            restoration: completed_pipe_product.restoration,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

impl From<CompletedPipeProduct> for ProductUpdateDocument {
    fn from(completed_pipe_product: CompletedPipeProduct) -> Self {
        let mut titles = completed_pipe_product.other_title;
        titles.insert(
            completed_pipe_product.native_title.language,
            completed_pipe_product.native_title.text,
        );
        let mut descriptions = completed_pipe_product.other_description;
        if let Some(description) = completed_pipe_product.native_description {
            descriptions.insert(description.language, description.text);
        }

        ProductUpdateDocument {
            event_id: None,
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            title_de: titles.remove(&LanguageRecord::De),
            title_en: titles.remove(&LanguageRecord::En),
            title_fr: titles.remove(&LanguageRecord::Fr),
            title_es: titles.remove(&LanguageRecord::Es),
            description_de: descriptions.remove(&LanguageRecord::De),
            description_en: descriptions.remove(&LanguageRecord::En),
            description_fr: descriptions.remove(&LanguageRecord::Fr),
            description_es: descriptions.remove(&LanguageRecord::Es),
            images: Some(
                completed_pipe_product
                    .images
                    .into_iter()
                    .map(ProductImageDocument::from)
                    .collect(),
            ),
            text_embedding: Some(completed_pipe_product.text_embedding),
            origin_year_min: completed_pipe_product.origin_year_min,
            origin_year: completed_pipe_product.origin_year,
            origin_year_max: completed_pipe_product.origin_year_max,
            authenticity: completed_pipe_product
                .authenticity
                .map(AuthenticityDocument::from),
            condition: completed_pipe_product
                .condition
                .map(ConditionDocument::from),
            provenance: completed_pipe_product
                .provenance
                .map(ProvenanceDocument::from),
            restoration: completed_pipe_product
                .restoration
                .map(RestorationDocument::from),
            updated: OffsetDateTime::now_utc(),
        }
    }
}
