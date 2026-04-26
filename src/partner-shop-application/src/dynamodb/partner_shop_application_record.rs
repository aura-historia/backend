use crate::core::{
    partner_shop_application::{
        CreateShopCommand, PartnerShopApplication, PartnerShopApplicationPayload,
    },
    partner_shop_application_id::PartnerShopApplicationId,
};
use crate::dynamodb::{
    partner_shop_application_payload_type_record::PartnerShopApplicationPayloadTypeRecord,
    partner_shop_application_state_record::PartnerShopApplicationStateRecord,
};
use common::execution_state::record::ExecutionStateRecord;
use common::{
    category_key::CategoryId, domain::Domain, period_key::PeriodId, shop_id::ShopId,
    shop_name::ShopName, user_id::UserId,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use shop::core::address::StructuredAddress;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct PartnerShopApplicationRecord {
    pub pk: String,
    pub sk: String,
    pub gsi1_pk: String,
    pub gsi1_sk: String,
    pub id: PartnerShopApplicationId,
    pub business_state: PartnerShopApplicationStateRecord,
    pub execution_state: ExecutionStateRecord,
    pub applicant_user_id: UserId,
    pub payload_type: PartnerShopApplicationPayloadTypeRecord,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub existing_shop_id: Option<ShopId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<ShopName>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_domains: Option<HashSet<Domain>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_image: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_structured_address: Option<StructuredAddress>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_email: Option<Email>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub shop_specialities_categories: Vec<CategoryId>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub shop_specialities_periods: Vec<PeriodId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_token: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user_id#{user_id}")
}

pub fn mk_sk(id: &PartnerShopApplicationId) -> String {
    format!("partner_shop_application_id#{id}")
}

pub fn mk_gsi1_pk() -> &'static str {
    "global#partner_shop_application"
}

pub fn mk_gsi1_sk(id: &PartnerShopApplicationId) -> String {
    format!("partner_shop_application_id#{id}")
}

impl From<PartnerShopApplication> for PartnerShopApplicationRecord {
    fn from(application: PartnerShopApplication) -> Self {
        let (
            payload_type,
            existing_shop_id,
            shop_name,
            shop_type,
            shop_domains,
            shop_image,
            shop_structured_address,
            shop_phone,
            shop_email,
            shop_specialities_categories,
            shop_specialities_periods,
        ) = match application.payload {
            PartnerShopApplicationPayload::Existing(shop_id) => (
                PartnerShopApplicationPayloadTypeRecord::Existing,
                Some(shop_id),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                Vec::new(),
            ),
            PartnerShopApplicationPayload::New(cmd) => (
                PartnerShopApplicationPayloadTypeRecord::New,
                None,
                Some(cmd.name),
                Some(cmd.shop_type.into()),
                Some(cmd.domains),
                cmd.image,
                cmd.structured_address,
                cmd.phone,
                cmd.email,
                cmd.specialities_categories,
                cmd.specialities_periods,
            ),
        };

        PartnerShopApplicationRecord {
            pk: mk_pk(&application.applicant_user_id),
            sk: mk_sk(&application.id),
            gsi1_pk: mk_gsi1_pk().to_owned(),
            gsi1_sk: mk_gsi1_sk(&application.id),
            id: application.id,
            business_state: application.business_state.into(),
            execution_state: application.execution_state.into(),
            applicant_user_id: application.applicant_user_id,
            payload_type,
            existing_shop_id,
            shop_name,
            shop_type,
            shop_domains,
            shop_image,
            shop_structured_address,
            shop_phone,
            shop_email,
            shop_specialities_categories,
            shop_specialities_periods,
            task_token: None,
            created: application.created,
            updated: application.updated,
        }
    }
}

impl TryFrom<PartnerShopApplicationRecord> for PartnerShopApplication {
    type Error = common::error::missing_field::MissingPersistenceField;

    fn try_from(record: PartnerShopApplicationRecord) -> Result<Self, Self::Error> {
        let payload = match record.payload_type {
            PartnerShopApplicationPayloadTypeRecord::Existing => {
                let shop_id = record.existing_shop_id.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("existing_shop_id")
                })?;
                PartnerShopApplicationPayload::Existing(shop_id)
            }
            PartnerShopApplicationPayloadTypeRecord::New => {
                let name = record.shop_name.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("shop_name")
                })?;
                let shop_type_record = record.shop_type.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("shop_type")
                })?;
                let domains = record.shop_domains.unwrap_or_default();
                let image = record.shop_image;

                PartnerShopApplicationPayload::New(CreateShopCommand {
                    name,
                    shop_type: shop_type_record.into(),
                    domains,
                    image,
                    structured_address: record.shop_structured_address,
                    phone: record.shop_phone,
                    email: record.shop_email,
                    specialities_categories: record.shop_specialities_categories,
                    specialities_periods: record.shop_specialities_periods,
                })
            }
        };

        Ok(PartnerShopApplication {
            id: record.id,
            business_state: record.business_state.into(),
            execution_state: record.execution_state.into(),
            applicant_user_id: record.applicant_user_id,
            payload,
            created: record.created,
            updated: record.updated,
        })
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::partner_shop_application::PartnerShopApplication;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PartnerShopApplicationRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let application: PartnerShopApplication = config.fake_with_rng(rng);
            PartnerShopApplicationRecord::from(application)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_partner_shop_application_record() {
            let _ = Faker.fake::<PartnerShopApplicationRecord>();
        }

        #[test]
        fn should_convert_domain_to_record_and_back_for_existing_payload() {
            let application = PartnerShopApplication {
                id: PartnerShopApplicationId::new(),
                business_state: crate::core::partner_shop_application_state::PartnerShopApplicationState::Submitted,
                execution_state: common::execution_state::ExecutionState::Processing,
                applicant_user_id: UserId::new(),
                payload: PartnerShopApplicationPayload::Existing(ShopId::new()),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let record = PartnerShopApplicationRecord::from(application.clone());
            let converted: PartnerShopApplication = record.try_into().unwrap();

            assert_eq!(application.id, converted.id);
            assert_eq!(application.business_state, converted.business_state);
            assert_eq!(application.execution_state, converted.execution_state);
            assert_eq!(application.applicant_user_id, converted.applicant_user_id);
            assert_eq!(application.payload, converted.payload);
        }

        #[test]
        fn should_convert_domain_to_record_and_back_for_new_payload() {
            use common::domain::Domain;
            use shop::core::shop_type::ShopType;

            let cmd = CreateShopCommand {
                name: ShopName::from("Test Shop".to_string()),
                shop_type: ShopType::CommercialDealer,
                domains: [Domain::try_from("https://www.test.com/".to_string()).unwrap()].into(),
                image: None,
                structured_address: None,
                phone: None,
                email: None,
                specialities_categories: Vec::new(),
                specialities_periods: Vec::new(),
            };

            let application = PartnerShopApplication {
                id: PartnerShopApplicationId::new(),
                business_state: crate::core::partner_shop_application_state::PartnerShopApplicationState::InReview,
                execution_state: common::execution_state::ExecutionState::Waiting,
                applicant_user_id: UserId::new(),
                payload: PartnerShopApplicationPayload::New(cmd),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let record = PartnerShopApplicationRecord::from(application.clone());
            let converted: PartnerShopApplication = record.try_into().unwrap();

            assert_eq!(application.id, converted.id);
            assert_eq!(application.business_state, converted.business_state);
            assert_eq!(application.execution_state, converted.execution_state);
            assert_eq!(application.applicant_user_id, converted.applicant_user_id);
            assert_eq!(application.payload, converted.payload);
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn should_format_pk_correctly() {
        let user_id = UserId::new();
        let pk = mk_pk(&user_id);
        assert_eq!(pk, format!("user_id#{user_id}"));
    }

    #[test]
    fn should_format_sk_correctly() {
        let id = PartnerShopApplicationId::new();
        let sk = mk_sk(&id);
        assert_eq!(sk, format!("partner_shop_application_id#{id}"));
    }

    #[test]
    fn should_format_gsi1_pk_correctly() {
        assert_eq!(mk_gsi1_pk(), "global#partner_shop_application");
    }

    #[test]
    fn should_format_gsi1_sk_correctly() {
        let id = PartnerShopApplicationId::new();
        let gsi1_sk = mk_gsi1_sk(&id);
        assert_eq!(gsi1_sk, format!("partner_shop_application_id#{id}"));
    }
}
