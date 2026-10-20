use localization::Language;
use money::Currency;
use schemars::JsonSchema;
use serde::Deserialize;

/// Existing presentation query used by listing detail and similarity reads.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
pub struct ProductListingPresentationData {
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    pub currency: Currency,
}
