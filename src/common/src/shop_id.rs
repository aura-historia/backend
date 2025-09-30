use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ShopId(Uuid);

impl Default for ShopId {
    fn default() -> Self {
        Self::new()
    }
}

impl ShopId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for ShopId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for ShopId {
    fn from(uuid: Uuid) -> Self {
        ShopId(uuid)
    }
}

impl TryFrom<String> for ShopId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<ShopId> for String {
    fn from(id: ShopId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for ShopId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

impl TryFrom<&String> for ShopId {
    type Error = uuid::Error;
    fn try_from(s: &String) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
        },
        shop_id::ShopId,
    };
    use std::collections::HashMap;

    pub fn extract_shop_id_path(path_params: &HashMap<String, String>) -> Result<ShopId, ApiError> {
        path_params
            .get("shopId")
            .map(ShopId::try_from)
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(INVALID_UUID)
                    .with_path_field("shopId")
                    .with_message(err.to_string())
            })?
            .ok_or(ApiError::bad_request(BAD_PATH_PARAMETER_VALUE).with_path_field("shopId"))
    }
}
