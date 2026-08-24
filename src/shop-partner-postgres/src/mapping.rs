use domain_primitives::versioned::Versioned;
use shop_core::shop_id::ShopId;
use shop_partner_core::partner_shop_application::{
    PartnerShopApplication, PartnerShopApplicationPayload, RehydratedPartnerShopApplicationState,
};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
use shop_partner_service::ports::{
    PartnerShopApplicationStorageVersion, PartnerShopApplicationView,
    VersionedPartnerShopApplication,
};
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use user_core::user_id::UserId;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
enum PartnerShopApplicationPayloadType {
    Existing,
    New,
}

impl PartnerShopApplicationPayloadType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "EXISTING",
            Self::New => "NEW",
        }
    }
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
        let payload = parse_payload(&row.payload_type, shop_id)?;
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
        let payload = parse_payload(&row.payload_type, shop_id)?;
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
    value.as_str()
}

pub(crate) fn bind_payload_type(value: PartnerShopApplicationPayload) -> &'static str {
    payload_type_for(value).as_str()
}

pub(crate) fn version_to_i64(
    version: PartnerShopApplicationStorageVersion,
) -> Result<i64, PartnerShopApplicationRowMappingError> {
    i64::try_from(version.into_inner())
        .map_err(|_| PartnerShopApplicationRowMappingError::InvalidVersion)
}

fn payload_type_for(payload: PartnerShopApplicationPayload) -> PartnerShopApplicationPayloadType {
    match payload {
        PartnerShopApplicationPayload::Existing { .. } => {
            PartnerShopApplicationPayloadType::Existing
        }
        PartnerShopApplicationPayload::New { .. } => PartnerShopApplicationPayloadType::New,
    }
}

fn parse_payload(
    value: &str,
    shop_id: ShopId,
) -> Result<PartnerShopApplicationPayload, PartnerShopApplicationRowMappingError> {
    match parse_payload_type(value)? {
        PartnerShopApplicationPayloadType::Existing => {
            Ok(PartnerShopApplicationPayload::Existing { shop_id })
        }
        PartnerShopApplicationPayloadType::New => {
            Ok(PartnerShopApplicationPayload::New { shop_id })
        }
    }
}

fn parse_payload_type(
    value: &str,
) -> Result<PartnerShopApplicationPayloadType, PartnerShopApplicationRowMappingError> {
    PartnerShopApplicationPayloadType::iter()
        .find(|payload_type| payload_type.as_str() == value)
        .ok_or(PartnerShopApplicationRowMappingError::InvalidPayloadType)
}

fn parse_business_state(
    value: &str,
) -> Result<PartnerShopApplicationState, PartnerShopApplicationRowMappingError> {
    PartnerShopApplicationState::from_code(value)
        .ok_or(PartnerShopApplicationRowMappingError::InvalidBusinessState)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_each_canonical_persisted_enum_identifier() {
        for expected in PartnerShopApplicationState::iter() {
            assert_eq!(Ok(expected), parse_business_state(expected.as_str()));
        }
        for expected in PartnerShopApplicationPayloadType::iter() {
            assert_eq!(Ok(expected), parse_payload_type(expected.as_str()));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_persisted_enum_identifiers() {
        assert_eq!(
            Err(PartnerShopApplicationRowMappingError::InvalidBusinessState),
            parse_business_state("submitted")
        );
        assert_eq!(
            Err(PartnerShopApplicationRowMappingError::InvalidPayloadType),
            parse_payload_type("unknown")
        );
    }

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
