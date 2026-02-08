use common::{
    category_key::{CategoryKey, CategorySlugId},
    string_newtype,
};
use time::OffsetDateTime;

string_newtype!(CategoryMetaName);
string_newtype!(CategoryMetaDescription);
string_newtype!(CategoryMetaKeyword);

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub category_key: CategoryKey,
    pub category_slug_id: CategorySlugId,
    pub meta_name: CategoryMetaName,
    pub meta_description: CategoryMetaDescription,
    pub meta_keywords: Vec<CategoryMetaKeyword>,
    pub embedding: Vec<f32>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Category {
    pub fn embedding_text(&self) -> String {
        format!(
            "{} [SEP] {} [SEP] {}",
            self.meta_name,
            self.meta_description,
            self.meta_keywords
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}
