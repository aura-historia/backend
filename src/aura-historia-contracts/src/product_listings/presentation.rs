use localization::Language;
use money::Currency;
use schemars::JsonSchema;
use serde::Deserialize;

/// Existing presentation query used by listing detail and similarity reads.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
pub struct ProductListingPresentationData {
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    #[schemars(description = crate::wire::description::LANGUAGE)]
    pub language: Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    #[schemars(description = crate::wire::description::CURRENCY)]
    pub currency: Currency,
}
