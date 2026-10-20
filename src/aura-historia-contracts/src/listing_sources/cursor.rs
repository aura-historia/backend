//! Query-bound public source continuation codec shared by REST and MCP.
use application::error::BoxError;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::queries::search_public_listing_sources::{
    PublicListingSourceSearchContinuation, PublicListingSourceSearchPosition,
};
use serde::{Deserialize, Serialize};

pub const MAX_CURSOR_ENCODED_BYTES: usize = 4 * 1024;
const MAX_CURSOR_DECODED_BYTES: usize = 3 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorData {
    binding: String,
    match_tier: u8,
    name_search: String,
    listing_source_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceCursorError {
    #[error("Search continuation exceeds the size limit.")]
    TooLarge,
    #[error("Search continuation is invalid.")]
    Invalid(#[source] BoxError),
    #[error("Search continuation is invalid.")]
    BindingLength,
    #[error("must be a valid ListingSource ID")]
    InvalidSourceId(#[source] domain_primitives::object_id::ObjectIdError),
}

pub fn parse_continuation(
    value: &str,
) -> Result<PublicListingSourceSearchContinuation, SourceCursorError> {
    if value.len() > MAX_CURSOR_ENCODED_BYTES {
        return Err(SourceCursorError::TooLarge);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|source| SourceCursorError::Invalid(Box::new(source)))?;
    if bytes.len() > MAX_CURSOR_DECODED_BYTES {
        return Err(SourceCursorError::TooLarge);
    }
    let data: CursorData = serde_json::from_slice(&bytes)
        .map_err(|source| SourceCursorError::Invalid(Box::new(source)))?;
    let binding = URL_SAFE_NO_PAD
        .decode(data.binding)
        .map_err(|source| SourceCursorError::Invalid(Box::new(source)))?
        .try_into()
        .map_err(|_: Vec<u8>| SourceCursorError::BindingLength)?;
    let listing_source_id: ListingSourceId = data
        .listing_source_id
        .parse()
        .map_err(SourceCursorError::InvalidSourceId)?;
    let position = PublicListingSourceSearchPosition::new(
        data.match_tier,
        data.name_search,
        listing_source_id,
    )
    .map_err(|source| SourceCursorError::Invalid(Box::new(source)))?;
    Ok(PublicListingSourceSearchContinuation::new(
        binding, position,
    ))
}

pub fn encode_continuation(
    continuation: &PublicListingSourceSearchContinuation,
) -> Result<String, serde_json::Error> {
    let position = continuation.position();
    let payload = CursorData {
        binding: URL_SAFE_NO_PAD.encode(continuation.binding()),
        match_tier: position.match_tier(),
        name_search: position.name_search().to_owned(),
        listing_source_id: position.listing_source_id().to_string(),
    };
    serde_json::to_vec(&payload).map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
}
