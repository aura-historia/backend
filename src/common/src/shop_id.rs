use uuid::Uuid;

crate::uuid_v4_newtype!(ShopId);

impl From<ShopId> for Uuid {
    fn from(id: ShopId) -> Self {
        id.0
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
        },
        error::missing_field::MissingRequiredField,
        shop_id::ShopId,
        shop_slug_id::ShopSlugId,
    };
    use std::collections::HashMap;

    pub fn extract_shop_id_path(path_params: &HashMap<String, String>) -> Result<ShopId, ApiError> {
        path_params
            .get("shopId")
            .map(ShopId::try_from)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_path_field("shopId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("shopId")),
                )
                .with_path_field("shopId")
                .with_detail("Missing field 'shopId'."),
            )
    }

    pub fn extract_shop_slug_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<ShopSlugId, ApiError> {
        path_params
            .get("shopSlugId")
            .map(ShopSlugId::raw)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PATH_PARAMETER_VALUE, Box::new(err))
                    .with_path_field("shopSlugId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("shopSlugId")),
                )
                .with_path_field("shopSlugId")
                .with_detail("Missing field 'shopSlugId'."),
            )
    }
}
