use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawExpectation {
    pub source_listing_id: String,
    pub title: String,
    pub description: Vec<String>,
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    pub state: String,
    pub images: Vec<String>,
}
