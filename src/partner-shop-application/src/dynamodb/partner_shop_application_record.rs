use crate::core::{
    partner_shop_application::{
        CreateShopCommand, PartnerShopApplication, PartnerShopApplicationPayload,
        PartnerShopApplicationState,
    },
    partner_shop_application_id::PartnerShopApplicationId,
};
use common::{shop_id::ShopId, user_id::UserId};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct PartnerShopApplicationRecord {
    pub pk: String,
    pub sk: String,
    pub gsi1_pk: String,
    pub gsi1_sk: String,
    pub id: PartnerShopApplicationId,
    pub state: PartnerShopApplicationStateRecord,
    pub applicant_user_id: UserId,
    pub payload_type: PartnerShopApplicationPayloadTypeRecord,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub existing_shop_id: Option<ShopId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_shop_command: Option<serde_json::Value>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PartnerShopApplicationStateRecord {
    Submitted,
    InReview,
    Rejected,
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PartnerShopApplicationPayloadTypeRecord {
    Existing,
    New,
}

impl From<PartnerShopApplicationState> for PartnerShopApplicationStateRecord {
    fn from(state: PartnerShopApplicationState) -> Self {
        match state {
            PartnerShopApplicationState::Submitted => PartnerShopApplicationStateRecord::Submitted,
            PartnerShopApplicationState::InReview => PartnerShopApplicationStateRecord::InReview,
            PartnerShopApplicationState::Rejected => PartnerShopApplicationStateRecord::Rejected,
            PartnerShopApplicationState::Approved => PartnerShopApplicationStateRecord::Approved,
        }
    }
}

impl From<PartnerShopApplicationStateRecord> for PartnerShopApplicationState {
    fn from(record: PartnerShopApplicationStateRecord) -> Self {
        match record {
            PartnerShopApplicationStateRecord::Submitted => PartnerShopApplicationState::Submitted,
            PartnerShopApplicationStateRecord::InReview => PartnerShopApplicationState::InReview,
            PartnerShopApplicationStateRecord::Rejected => PartnerShopApplicationState::Rejected,
            PartnerShopApplicationStateRecord::Approved => PartnerShopApplicationState::Approved,
        }
    }
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
        let (payload_type, existing_shop_id, new_shop_command) = match &application.payload {
            PartnerShopApplicationPayload::Existing(shop_id) => (
                PartnerShopApplicationPayloadTypeRecord::Existing,
                Some(*shop_id),
                None,
            ),
            PartnerShopApplicationPayload::New(cmd) => (
                PartnerShopApplicationPayloadTypeRecord::New,
                None,
                Some(serialize_create_shop_command(cmd)),
            ),
        };

        PartnerShopApplicationRecord {
            pk: mk_pk(&application.applicant_user_id),
            sk: mk_sk(&application.id),
            gsi1_pk: mk_gsi1_pk().to_owned(),
            gsi1_sk: mk_gsi1_sk(&application.id),
            id: application.id,
            state: application.state.into(),
            applicant_user_id: application.applicant_user_id,
            payload_type,
            existing_shop_id,
            new_shop_command,
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
                let cmd_value = record.new_shop_command.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("new_shop_command")
                })?;
                let cmd = deserialize_create_shop_command(cmd_value).map_err(|_| {
                    common::error::missing_field::MissingPersistenceField::new(
                        "new_shop_command (deserialization failed)",
                    )
                })?;
                PartnerShopApplicationPayload::New(cmd)
            }
        };

        Ok(PartnerShopApplication {
            id: record.id,
            state: record.state.into(),
            applicant_user_id: record.applicant_user_id,
            payload,
            created: record.created,
            updated: record.updated,
        })
    }
}

fn serialize_create_shop_command(cmd: &CreateShopCommand) -> serde_json::Value {
    use common::domain::Domain;
    serde_json::json!({
        "name": String::from(cmd.name.clone()),
        "shop_type": format!("{:?}", cmd.shop_type),
        "domains": cmd.domains.iter().map(|d: &Domain| d.to_string()).collect::<Vec<_>>(),
        "image": cmd.image.as_ref().map(|u: &url::Url| u.to_string()),
    })
}

fn deserialize_create_shop_command(value: serde_json::Value) -> Result<CreateShopCommand, String> {
    use common::{domain::Domain, shop_name::ShopName};
    use shop::core::shop_type::ShopType;
    use std::collections::HashSet;

    let obj = value.as_object().ok_or("expected object")?;

    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| ShopName::from(s.to_string()))
        .ok_or("missing name")?;

    let shop_type = obj
        .get("shop_type")
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "AuctionHouse" => Some(ShopType::AuctionHouse),
            "AuctionPlatform" => Some(ShopType::AuctionPlatform),
            "CommercialDealer" => Some(ShopType::CommercialDealer),
            "Marketplace" => Some(ShopType::Marketplace),
            _ => None,
        })
        .ok_or("missing or invalid shop_type")?;

    let domains: HashSet<Domain> = obj
        .get("domains")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| Domain::try_from(s.to_string()).ok())
                .collect()
        })
        .unwrap_or_default();

    let image = obj
        .get("image")
        .and_then(|v| v.as_str())
        .and_then(|s| url::Url::parse(s).ok());

    Ok(CreateShopCommand {
        name,
        shop_type,
        domains,
        image,
    })
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PartnerShopApplicationRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let application: PartnerShopApplication = config.fake_with_rng(rng);
            PartnerShopApplicationRecord::from(application)
        }
    }

    impl Dummy<Faker> for PartnerShopApplicationStateRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let index: u8 = config.fake_with_rng(rng);
            match index % 4 {
                0 => PartnerShopApplicationStateRecord::Submitted,
                1 => PartnerShopApplicationStateRecord::InReview,
                2 => PartnerShopApplicationStateRecord::Rejected,
                _ => PartnerShopApplicationStateRecord::Approved,
            }
        }
    }

    impl Dummy<Faker> for PartnerShopApplicationPayloadTypeRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            if config.fake_with_rng::<bool, R>(rng) {
                PartnerShopApplicationPayloadTypeRecord::Existing
            } else {
                PartnerShopApplicationPayloadTypeRecord::New
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::core::partner_shop_application::PartnerShopApplication;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_partner_shop_application_record() {
            let _ = Faker.fake::<PartnerShopApplicationRecord>();
        }

        #[test]
        fn should_convert_domain_to_record_and_back_for_existing_payload() {
            let application = PartnerShopApplication {
                id: PartnerShopApplicationId::new(),
                state: PartnerShopApplicationState::Submitted,
                applicant_user_id: UserId::new(),
                payload: PartnerShopApplicationPayload::Existing(ShopId::new()),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let record = PartnerShopApplicationRecord::from(application.clone());
            let converted: PartnerShopApplication = record.try_into().unwrap();

            assert_eq!(application.id, converted.id);
            assert_eq!(application.state, converted.state);
            assert_eq!(application.applicant_user_id, converted.applicant_user_id);
            assert_eq!(application.payload, converted.payload);
        }

        #[test]
        fn should_convert_domain_to_record_and_back_for_new_payload() {
            use common::{domain::Domain, shop_name::ShopName};
            use shop::core::shop_type::ShopType;

            let cmd = CreateShopCommand {
                name: ShopName::from("Test Shop".to_string()),
                shop_type: ShopType::CommercialDealer,
                domains: [Domain::try_from("https://www.test.com/".to_string()).unwrap()].into(),
                image: None,
            };

            let application = PartnerShopApplication {
                id: PartnerShopApplicationId::new(),
                state: PartnerShopApplicationState::InReview,
                applicant_user_id: UserId::new(),
                payload: PartnerShopApplicationPayload::New(cmd),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let record = PartnerShopApplicationRecord::from(application.clone());
            let converted: PartnerShopApplication = record.try_into().unwrap();

            assert_eq!(application.id, converted.id);
            assert_eq!(application.state, converted.state);
            assert_eq!(application.applicant_user_id, converted.applicant_user_id);
            assert_eq!(application.payload, converted.payload);
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use common::user_id::UserId;

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

    #[test]
    fn should_convert_state_domain_to_record_and_back() {
        let states = [
            PartnerShopApplicationState::Submitted,
            PartnerShopApplicationState::InReview,
            PartnerShopApplicationState::Rejected,
            PartnerShopApplicationState::Approved,
        ];
        for state in states {
            let record: PartnerShopApplicationStateRecord = state.into();
            let converted: PartnerShopApplicationState = record.into();
            assert_eq!(state, converted);
        }
    }
}
