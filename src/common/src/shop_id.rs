use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};
use url::Url;
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub enum ShopIdentifier {
    ShopId(ShopId),
    ShopUrl(Url),
}

impl fmt::Display for ShopIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShopIdentifier::ShopId(id) => write!(f, "{id}"),
            ShopIdentifier::ShopUrl(url) => write!(f, "{url}"),
        }
    }
}

impl From<ShopId> for ShopIdentifier {
    fn from(shop_id: ShopId) -> Self {
        Self::ShopId(shop_id)
    }
}

impl From<Url> for ShopIdentifier {
    fn from(url: Url) -> Self {
        Self::ShopUrl(url)
    }
}

impl From<ShopIdentifier> for String {
    fn from(value: ShopIdentifier) -> Self {
        match value {
            ShopIdentifier::ShopId(shop_id) => shop_id.to_string(),
            ShopIdentifier::ShopUrl(url) => url.to_string(),
        }
    }
}

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
        error::missing_field::MissingRequiredField,
        shop_id::ShopId,
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
}
