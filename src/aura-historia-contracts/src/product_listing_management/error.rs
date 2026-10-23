use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct MutationInputError {
    pub field: &'static str,
    pub detail: String,
    pub invalid_object_id: bool,
    #[source]
    pub source: Option<BoxError>,
}
impl MutationInputError {
    pub(crate) fn invalid(field: &'static str, detail: impl Into<String>) -> Self {
        Self {
            field,
            detail: detail.into(),
            invalid_object_id: false,
            source: None,
        }
    }
    pub(crate) fn caused(
        field: &'static str,
        detail: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            source: Some(Box::new(source)),
            ..Self::invalid(field, detail)
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct QueryInputError {
    pub field: &'static str,
    pub detail: String,
    pub invalid_object_id: bool,
}
impl QueryInputError {
    pub(crate) fn invalid(field: &'static str, detail: impl Into<String>) -> Self {
        Self {
            field,
            detail: detail.into(),
            invalid_object_id: false,
        }
    }
}
