use common::{
    language::record::{LanguageRecord, TextRecord},
    product_id::ProductId,
    shop_id::ShopId,
    shop_name::ShopName,
    shops_product_id::ShopsProductId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub trait HasProductId {
    fn product_id(&self) -> ProductId;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleansedPipeProduct {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub native_description: Option<TextRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedPipeProduct {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEmbeddedPipeProduct {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub text_embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedPipeProduct {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub text_embedding: Vec<f32>,
}
