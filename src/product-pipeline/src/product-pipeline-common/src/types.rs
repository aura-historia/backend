use common::{
    language::record::{LanguageRecord, TextRecord},
    product_id::ProductId,
    shop_id::ShopId,
    shops_product_id::ShopsProductId,
};
use product::{
    dynamodb::product_update_record::ProductRecordUpdate,
    opensearch::product_update_document::ProductUpdateDocument,
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
    pub text_embedding: Vec<f32>,
}

impl HasProductId for TextEmbeddedPipeProduct {
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
    pub text_embedding: Vec<f32>,
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
            text_embedding: Some(completed_pipe_product.text_embedding),
            updated: OffsetDateTime::now_utc(),
        }
    }
}
