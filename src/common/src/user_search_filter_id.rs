// Legacy shim. Owner: search-filter-core. Remove after legacy common consumers migrate.
pub type UserSearchFilterId = search_filter_core::user_search_filter_id::UserSearchFilterId;

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
        },
        error::missing_field::MissingRequiredField,
        user_search_filter_id::UserSearchFilterId,
    };
    use std::collections::HashMap;

    pub fn extract_user_search_filter_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<UserSearchFilterId, ApiError> {
        path_params
            .get("userSearchFilterId")
            .map(UserSearchFilterId::try_from)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_path_field("userSearchFilterId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("userSearchFilterId")),
                )
                .with_path_field("userSearchFilterId")
                .with_detail("Missing field 'userSearchFilterId'."),
            )
    }
}
