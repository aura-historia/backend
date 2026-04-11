use crate::{
    core::{
        partner_shop_application::{PartnerShopApplication, PartnerShopApplicationPayload},
        partner_shop_application_id::PartnerShopApplicationId,
    },
    data::partner_shop_application_state_data::PartnerShopApplicationStateData,
};
use common::execution_state::data::ExecutionStateData;
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPartnerShopApplicationData {
    pub id: PartnerShopApplicationId,
    pub business_state: PartnerShopApplicationStateData,
    pub execution_state: ExecutionStateData,
    pub payload: GetPartnerShopApplicationPayloadData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum GetPartnerShopApplicationPayloadData {
    #[serde(rename = "EXISTING")]
    Existing { shop_id: ShopId },
    #[serde(rename = "NEW")]
    New {
        shop_name: ShopName,
        shop_type: ShopTypeData,
        shop_domains: HashSet<Domain>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_image: Option<Url>,
    },
}

impl From<PartnerShopApplication> for GetPartnerShopApplicationData {
    fn from(application: PartnerShopApplication) -> Self {
        let payload = match application.payload {
            PartnerShopApplicationPayload::Existing(shop_id) => {
                GetPartnerShopApplicationPayloadData::Existing { shop_id }
            }
            PartnerShopApplicationPayload::New(cmd) => GetPartnerShopApplicationPayloadData::New {
                shop_name: cmd.name,
                shop_type: cmd.shop_type.into(),
                shop_domains: cmd.domains,
                shop_image: cmd.image,
            },
        };

        GetPartnerShopApplicationData {
            id: application.id,
            business_state: application.business_state.into(),
            execution_state: application.execution_state.into(),
            payload,
            created: application.created,
            updated: application.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::partner_shop_application::PartnerShopApplication;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for GetPartnerShopApplicationData {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            config
                .fake_with_rng::<PartnerShopApplication, R>(rng)
                .into()
        }
    }
}
