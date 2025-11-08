use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShopsItemId(String);

impl ShopsItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ShopsItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ShopsItemId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<ShopsItemId> for String {
    fn from(id: ShopsItemId) -> Self {
        id.0
    }
}

impl From<String> for ShopsItemId {
    fn from(value: String) -> Self {
        ShopsItemId(value)
    }
}

impl From<&String> for ShopsItemId {
    fn from(value: &String) -> Self {
        ShopsItemId(value.to_owned())
    }
}

impl From<&str> for ShopsItemId {
    fn from(value: &str) -> Self {
        ShopsItemId(value.to_owned())
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_PATH_PARAMETER_VALUE},
        error::missing_field::MissingRequiredField,
        shops_item_id::ShopsItemId,
    };
    use std::collections::HashMap;

    pub fn extract_shops_item_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<ShopsItemId, ApiError> {
        path_params
            .get("shopsItemId")
            .filter(|str| !str.is_empty())
            .map(ShopsItemId::from)
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("shopsItemId")),
                )
                .with_path_field("shopsItemId")
                .with_message("Missing field 'shopsItemId'."),
            )
    }
}
