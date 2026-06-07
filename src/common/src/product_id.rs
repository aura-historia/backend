use crate::shop_id::ShopId;
use crate::shops_product_id::ShopsProductId;
use std::fmt::{Display, Formatter};

crate::uuid_v4_newtype!(ProductId);

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct ProductKey {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
}

impl ProductKey {
    pub fn new(shop_id: ShopId, shops_product_id: ShopsProductId) -> Self {
        ProductKey {
            shop_id,
            shops_product_id,
        }
    }
}

impl From<ProductKey> for String {
    fn from(key: ProductKey) -> Self {
        format!(
            "shop_id#{}#shops_product_id#{}",
            key.shop_id, key.shops_product_id
        )
    }
}

impl Display for ProductKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "shop_id#{}#shops_product_id#{}",
            self.shop_id, self.shops_product_id
        )
    }
}

impl TryFrom<&str> for ProductKey {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if let Some((shop_id, shops_product_id)) = value
            .trim_start_matches("shop_id#")
            .split_once("#shops_product_id#")
        {
            Ok(ProductKey {
                shop_id: shop_id
                    .try_into()
                    .map_err(|err: uuid::Error| err.to_string())?,
                shops_product_id: shops_product_id.into(),
            })
        } else {
            Err(format!("Parsing ProductKey '{value}' failed."))
        }
    }
}

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
