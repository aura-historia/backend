use common::partner_shop_application_id::PartnerShopApplicationId;
use common::shop_id::ShopId;
use common::user_id::UserId;
use serde::{Deserialize, Serialize};
use shop_partner_core::partner_shop_application::{
    PartnerShopApplication, PartnerShopApplicationPayload,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartnerApplicationData {
    pub(crate) id: PartnerShopApplicationId,
    pub(crate) applicant_user_id: UserId,
    pub(crate) business_state: String,
    pub(crate) execution_state: String,
    pub(crate) payload: PartnerApplicationPayloadData,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum PartnerApplicationPayloadData {
    Existing { shop_id: ShopId },
    New { shop_id: ShopId },
}
impl From<PartnerShopApplication> for PartnerApplicationData {
    fn from(a: PartnerShopApplication) -> Self {
        let payload = match a.payload() {
            PartnerShopApplicationPayload::Existing { shop_id } => {
                PartnerApplicationPayloadData::Existing { shop_id }
            }
            PartnerShopApplicationPayload::New { shop_id } => {
                PartnerApplicationPayloadData::New { shop_id }
            }
        };
        Self {
            id: a.id(),
            applicant_user_id: a.applicant_user_id(),
            business_state: format!("{:?}", a.business_state()),
            execution_state: format!("{:?}", a.execution_state()),
            payload,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum PostPayloadData {
    Existing { shop_id: ShopId },
    New { shop_id: ShopId },
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostApplicationData {
    pub(crate) payload: PostPayloadData,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchApplicationData {
    pub(crate) task_token: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecisionData {
    pub(crate) decision: String,
}
