use common::{domain::Domain, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use url::Url;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostShopData {
    pub name: ShopName,
    pub domains: HashSet<Domain>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,
}
