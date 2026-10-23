use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, INVALID_OBJECT_ID};
use aura_historia_contracts::product_listing_management::QueryInputError;
pub(crate) use aura_historia_contracts::product_listing_management::{SearchQuery, SearchResponse};
impl From<QueryInputError> for ApiError {
    fn from(error: QueryInputError) -> Self {
        Self::bad_request(if error.invalid_object_id {
            INVALID_OBJECT_ID
        } else {
            BAD_QUERY_PARAMETER_VALUE
        })
        .with_query_field(error.field)
        .with_detail(error.detail)
    }
}
