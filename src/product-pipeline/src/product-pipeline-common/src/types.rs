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

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleansedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub native_description: Option<TextRecord>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEmbeddedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub text_embedding: Vec<f32>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedPipeProduct {
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: TextRecord,
    pub other_title: HashMap<LanguageRecord, String>,
    pub native_description: Option<TextRecord>,
    pub other_description: HashMap<LanguageRecord, String>,
    pub text_embedding: Vec<f32>,
}
