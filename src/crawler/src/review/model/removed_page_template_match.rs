use super::SchemaPageReference;
use serde::{Deserialize, Serialize};

/// A review page the candidate's removed-page templates match, and what the scraper would
/// make of it once the candidate is live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedPageTemplateMatch {
    pub page_reference: SchemaPageReference,
    pub url: String,
    pub role: String,
    pub outcome: RemovedPageTemplateOutcome,
    pub templates: Vec<MatchedRemovedPageTemplate>,
}

/// The scraper's verdict on a page a removed-page template matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RemovedPageTemplateOutcome {
    /// No candidate schema extracts a product from the page: it is captured as removed.
    Removed,
    /// A candidate schema still extracts a product with a title other than the notice, so the
    /// page is scraped as a product and the template is over-broad for it.
    ProductStillExtracts,
}

/// One template that matched the page, by its index in the candidate's
/// `removed_page_schemas`, with the normalized page texts that carried its notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedRemovedPageTemplate {
    pub template_index: usize,
    pub notices: Vec<String>,
}
