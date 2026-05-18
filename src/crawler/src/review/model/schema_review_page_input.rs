#[derive(Debug, Clone)]
pub struct SchemaReviewPageInput {
    pub url: String,
    pub role: String,
    pub raw_html: String,
}
