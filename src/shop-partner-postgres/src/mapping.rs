use common::{partner_shop_application_id::PartnerShopApplicationId, user_id::UserId};
use domain_primitives::versioned::Versioned;
use shop_core::shop_id::ShopId;
use shop_partner_core::partner_shop_application::{
    PartnerShopApplication, PartnerShopApplicationPayload, RehydratedPartnerShopApplicationState,
};
use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
use shop_partner_service::ports::{
    PartnerShopApplicationStorageVersion, PartnerShopApplicationView,
    VersionedPartnerShopApplication,
};
use time::OffsetDateTime;

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct PartnerShopApplicationRow {
    pub partner_shop_application_id: uuid::Uuid,
    pub applicant_user_id: uuid::Uuid,
    pub business_state: String,
    pub payload_type: String,
    pub shop_id: uuid::Uuid,
    pub version: i64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum PartnerShopApplicationRowMappingError {
    #[error("invalid partner shop application business state persisted")]
    InvalidBusinessState,
    #[error("invalid partner shop application payload type persisted")]
    InvalidPayloadType,

    #[error("invalid partner shop application version persisted")]
    InvalidVersion,
}

pub(crate) const APPLICATION_COLUMNS: &str = r#"
    partner_shop_application_id, applicant_user_id, business_state,
    payload_type, shop_id, version, created, updated
"#;

impl TryFrom<PartnerShopApplicationRow> for PartnerShopApplicationView {
    type Error = PartnerShopApplicationRowMappingError;

    fn try_from(row: PartnerShopApplicationRow) -> Result<Self, Self::Error> {
        let shop_id = ShopId::from(row.shop_id);
        let payload = match row.payload_type.as_str() {
            "EXISTING" => PartnerShopApplicationPayload::Existing { shop_id },
            "NEW" => PartnerShopApplicationPayload::New { shop_id },
            _ => return Err(PartnerShopApplicationRowMappingError::InvalidPayloadType),
        };
        Ok(Self {
            id: PartnerShopApplicationId::from(row.partner_shop_application_id),
            applicant_user_id: UserId::from(row.applicant_user_id),
            business_state: parse_business_state(&row.business_state)?,
            payload,
            shop_id,
        })
    }
}

impl TryFrom<PartnerShopApplicationRow> for VersionedPartnerShopApplication {
    type Error = PartnerShopApplicationRowMappingError;

    fn try_from(row: PartnerShopApplicationRow) -> Result<Self, Self::Error> {
        let version = PartnerShopApplicationStorageVersion::try_from(row.version)
            .map_err(|_| PartnerShopApplicationRowMappingError::InvalidVersion)?;
        let shop_id = ShopId::from(row.shop_id);
        let payload = match row.payload_type.as_str() {
            "EXISTING" => PartnerShopApplicationPayload::Existing { shop_id },
            "NEW" => PartnerShopApplicationPayload::New { shop_id },
            _ => return Err(PartnerShopApplicationRowMappingError::InvalidPayloadType),
        };
        let application =
            PartnerShopApplication::rehydrate(RehydratedPartnerShopApplicationState {
                id: PartnerShopApplicationId::from(row.partner_shop_application_id),
                applicant_user_id: UserId::from(row.applicant_user_id),
                business_state: parse_business_state(&row.business_state)?,
                payload,
            });
        Ok(Versioned::new(application, version))
    }
}

pub(crate) fn bind_business_state(value: PartnerShopApplicationState) -> &'static str {
    match value {
        PartnerShopApplicationState::Submitted => "SUBMITTED",
        PartnerShopApplicationState::InReview => "IN_REVIEW",
        PartnerShopApplicationState::Rejected => "REJECTED",
        PartnerShopApplicationState::Approved => "APPROVED",
        PartnerShopApplicationState::Withdrawn => "WITHDRAWN",
    }
}

pub(crate) fn bind_payload_type(value: PartnerShopApplicationPayload) -> &'static str {
    match value {
        PartnerShopApplicationPayload::Existing { .. } => "EXISTING",
        PartnerShopApplicationPayload::New { .. } => "NEW",
    }
}

pub(crate) fn version_to_i64(
    version: PartnerShopApplicationStorageVersion,
) -> Result<i64, PartnerShopApplicationRowMappingError> {
    i64::try_from(version.into_inner())
        .map_err(|_| PartnerShopApplicationRowMappingError::InvalidVersion)
}

fn parse_business_state(
    value: &str,
) -> Result<PartnerShopApplicationState, PartnerShopApplicationRowMappingError> {
    match value {
        "SUBMITTED" => Ok(PartnerShopApplicationState::Submitted),
        "IN_REVIEW" => Ok(PartnerShopApplicationState::InReview),
        "REJECTED" => Ok(PartnerShopApplicationState::Rejected),
        "APPROVED" => Ok(PartnerShopApplicationState::Approved),
        "WITHDRAWN" => Ok(PartnerShopApplicationState::Withdrawn),
        _ => Err(PartnerShopApplicationRowMappingError::InvalidBusinessState),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_storage_version_that_cannot_fit_postgres_integer() {
        let version = PartnerShopApplicationStorageVersion::try_from(u64::MAX)
            .map_err(|_| PartnerShopApplicationRowMappingError::InvalidVersion);

        assert!(matches!(
            version.and_then(version_to_i64),
            Err(PartnerShopApplicationRowMappingError::InvalidVersion)
        ));
    }
}
