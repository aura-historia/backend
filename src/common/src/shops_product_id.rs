// Legacy shim. Owner: product-core. Remove after legacy Product consumers migrate.
pub use product_core::shops_product_id::ShopsProductId;

impl From<crate::slug_id::SlugId<0>> for ShopsProductId {
    fn from(value: crate::slug_id::SlugId<0>) -> Self {
        Self::from(value.to_string())
    }
}

impl From<ShopsProductId> for crate::slug_id::SlugId<0> {
    fn from(value: ShopsProductId) -> Self {
        Self::from(value.to_string())
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_PATH_PARAMETER_VALUE},
        error::missing_field::MissingRequiredField,
        shops_product_id::ShopsProductId,
    };
    use std::collections::HashMap;

    pub fn extract_shops_product_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<ShopsProductId, ApiError> {
        path_params
            .get("shopsProductId")
            .filter(|value| !value.is_empty())
            .map(ShopsProductId::raw)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PATH_PARAMETER_VALUE, Box::new(err))
                    .with_path_field("shopsProductId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("shopsProductId")),
                )
                .with_path_field("shopsProductId")
                .with_detail("Missing field 'shopsProductId'."),
            )
    }
}
