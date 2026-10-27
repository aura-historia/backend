use crate::scraper::css_selector::raw_attribute_registry::registered_definitions;
use serde::Serialize;

/// A raw attribute key the review console offers for a schema's raw attributes, as
/// `GET /api/raw-attribute-keys` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawAttributeKey {
    pub key: &'static str,
    pub group: &'static str,
    pub description: &'static str,
}

impl RawAttributeKey {
    /// Every registered key, in registry order.
    pub fn registry() -> Vec<Self> {
        registered_definitions()
            .map(|(group, definition)| Self {
                key: definition.key,
                group,
                description: definition.description,
            })
            .collect()
    }
}
