use crate::domain::Domain;
use std::fmt;
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShopIdentifier {
    ShopId(ShopId),
    ShopDomain(Domain),
}

impl fmt::Display for ShopIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShopIdentifier::ShopId(id) => write!(f, "{id}"),
            ShopIdentifier::ShopDomain(domain) => write!(f, "{domain}"),
        }
    }
}

impl From<ShopId> for ShopIdentifier {
    fn from(shop_id: ShopId) -> Self {
        Self::ShopId(shop_id)
    }
}

impl From<Domain> for ShopIdentifier {
    fn from(domain: Domain) -> Self {
        Self::ShopDomain(domain)
    }
}

impl From<ShopIdentifier> for String {
    fn from(value: ShopIdentifier) -> Self {
        match value {
            ShopIdentifier::ShopId(shop_id) => shop_id.to_string(),
            ShopIdentifier::ShopDomain(domain) => domain.to_string(),
        }
    }
}

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
        slug_id::SlugId,
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
    ) -> Result<SlugId<0>, ApiError> {
        path_params.get("shopSlugId").map(SlugId::raw).ok_or(
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("shopSlugId")),
            )
            .with_path_field("shopSlugId")
            .with_detail("Missing field 'shopSlugId'."),
        )
    }
}
