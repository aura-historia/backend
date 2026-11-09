pub const ARTIFACT_URL_PATTERN: &str = "URL_PATTERN";
pub const ARTIFACT_PRODUCT_SCHEMA: &str = "PRODUCT_SCHEMA";

/// A review's artifact type as a console list names it in its `artifact_type` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactType {
    UrlPattern,
    ProductSchema,
}

impl ArtifactType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UrlPattern => ARTIFACT_URL_PATTERN,
            Self::ProductSchema => ARTIFACT_PRODUCT_SCHEMA,
        }
    }
}

impl super::QueryValue for ArtifactType {
    const VALUES: &'static [(&'static str, Self)] = &[
        (ARTIFACT_URL_PATTERN, Self::UrlPattern),
        (ARTIFACT_PRODUCT_SCHEMA, Self::ProductSchema),
    ];
}
