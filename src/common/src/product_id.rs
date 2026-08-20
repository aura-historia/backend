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

#[cfg(test)]
mod tests {
    use rstest;

    #[rstest::rstest]
    #[trace]
    #[case::differing(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::product_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152")]
    #[case::product_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152-49874651-845")]
    fn should_display_product_key(#[case] shop_id: String, #[case] shops_product_id: &str) {
        use crate::product_id::ProductKey;

        let expected = format!("shop_id#{shop_id}#shops_product_id#{shops_product_id}");

        let actual = ProductKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_product_id: shops_product_id.into(),
        }
        .to_string();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[trace]
    #[case::differing(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::product_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152")]
    #[case::product_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152-49874651-845")]
    fn should_into_string_product_key(#[case] shop_id: String, #[case] shops_product_id: &str) {
        use crate::product_id::ProductKey;

        let expected = format!("shop_id#{shop_id}#shops_product_id#{shops_product_id}");

        let actual: String = ProductKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_product_id: shops_product_id.into(),
        }
        .into();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[trace]
    #[case::differing(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::product_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152")]
    #[case::product_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152-49874651-845")]
    fn should_parse_product_key(#[case] shop_id: String, #[case] shops_product_id: &str) {
        use crate::product_id::ProductKey;

        let payload = format!("shop_id#{shop_id}#shops_product_id#{shops_product_id}");
        let actual = ProductKey::try_from(payload.as_str());

        let expected = ProductKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_product_id: shops_product_id.into(),
        };

        assert_eq!(expected, actual.unwrap());
    }
}
