// Legacy shim. Owner: product-core. Remove after legacy Product consumers migrate.
pub use product_core::product_id::{ProductId, ProductKey};

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_PATH_PARAMETER_VALUE},
        error::missing_field::MissingRequiredField,
        product_slug_id::ProductSlugId,
        shop_id::ShopId,
        shops_product_id::ShopsProductId,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    #[cfg_attr(feature = "test-data", derive(fake::Dummy))]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProductKeyData {
        pub shop_id: ShopId,
        pub shops_product_id: ShopsProductId,
    }

    pub fn extract_product_slug_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<ProductSlugId, ApiError> {
        path_params
            .get("productSlugId")
            .map(ProductSlugId::raw)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PATH_PARAMETER_VALUE, Box::new(err))
                    .with_path_field("productSlugId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("productSlugId")),
                )
                .with_path_field("productSlugId")
                .with_detail("Missing field 'productSlugId'."),
            )
    }
}
