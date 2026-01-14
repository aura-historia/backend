use crate::data::shop_type_data::ShopTypeData;
use common::{domain::Domain, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use url::Url;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchShopData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<ShopName>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,
}
