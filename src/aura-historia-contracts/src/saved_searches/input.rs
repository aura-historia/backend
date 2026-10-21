use crate::patch_value::PatchValue;
use application::patch_field::PatchField;
use std::str::FromStr;

#[derive(Debug, Clone, Copy)]
pub enum SavedSearchInputErrorKind {
    BodyValue,
    QueryValue,
    Identifier,
}

/// Shared validation information; transports retain their own status/envelope mapping.
#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct SavedSearchInputError {
    pub kind: SavedSearchInputErrorKind,
    pub field: Option<&'static str>,
    pub query: bool,
    pub detail: String,
}
impl SavedSearchInputError {
    pub(super) fn new(kind: SavedSearchInputErrorKind) -> Self {
        Self {
            kind,
            field: None,
            query: false,
            detail: String::new(),
        }
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            SavedSearchInputErrorKind::BodyValue => "BAD_BODY_VALUE",
            SavedSearchInputErrorKind::QueryValue => "BAD_QUERY_PARAMETER_VALUE",
            SavedSearchInputErrorKind::Identifier => "INVALID_OBJECT_ID",
        }
    }
    pub(super) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
    pub(super) fn with_body_field(mut self, field: &'static str) -> Self {
        self.field = Some(field);
        self
    }
    pub(super) fn with_query_field(mut self, field: &'static str) -> Self {
        self.field = Some(field);
        self.query = true;
        self
    }
}
pub(super) fn non_nullable_patch<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<PatchField<T>, SavedSearchInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Value(value) => Ok(PatchField::Set(value)),
        PatchValue::Null => Err(
            SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue)
                .with_detail(format!("Body field '{field}' must not be null.")),
        ),
    }
}
pub(super) fn parse_body_object_id<T: FromStr>(
    value: &str,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<T, SavedSearchInputError> {
    value.parse().map_err(|_| {
        SavedSearchInputError::new(SavedSearchInputErrorKind::Identifier)
            .with_body_field(field)
            .with_detail(format!("must be a valid {semantic_type} ID"))
    })
}
