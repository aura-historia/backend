use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShopsProductId(String);

impl ShopsProductId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ShopsProductId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ShopsProductId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<ShopsProductId> for String {
    fn from(id: ShopsProductId) -> Self {
        id.0
    }
}

impl From<String> for ShopsProductId {
    fn from(value: String) -> Self {
        ShopsProductId(value)
    }
}

impl From<&String> for ShopsProductId {
    fn from(value: &String) -> Self {
        ShopsProductId(value.to_owned())
    }
}

impl From<&str> for ShopsProductId {
    fn from(value: &str) -> Self {
        ShopsProductId(value.to_owned())
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
            .filter(|str| !str.is_empty())
            .map(ShopsProductId::from)
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
