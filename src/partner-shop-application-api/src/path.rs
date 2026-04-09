use common::api::error::ApiError;
use common::api::error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID};
use common::error::missing_field::MissingRequiredField;
use partner_shop_application::core::partner_shop_application_id::PartnerShopApplicationId;
use std::collections::HashMap;

pub fn extract_partner_application_id_path(
    path_params: &HashMap<String, String>,
) -> Result<PartnerShopApplicationId, ApiError> {
    path_params
        .get("partnerApplicationId")
        .map(PartnerShopApplicationId::try_from)
        .transpose()
        .map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_path_field("partnerApplicationId")
                .with_detail(msg)
        })?
        .ok_or(
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("partnerApplicationId")),
            )
            .with_path_field("partnerApplicationId")
            .with_detail("Missing field 'partnerApplicationId'."),
        )
}
