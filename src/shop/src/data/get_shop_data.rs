use crate::core::address::{GeoAddress, StructuredAddress};
use crate::{
    core::shop::Shop,
    data::{partner_status_data::ShopPartnerStatusData, shop_type_data::ShopTypeData},
};
use common::{
    category_key::CategoryId, domain::Domain, period_key::PeriodId, shop_id::ShopId,
    shop_name::ShopName, slug_id::SlugId,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetShopData {
    pub shop_id: ShopId,
    pub shop_slug_id: SlugId<0>,
    pub name: ShopName,
    pub shop_type: ShopTypeData,
    pub domains: HashSet<Domain>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddress>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<GeoAddress>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub specialities_categories: Vec<CategoryId>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub specialities_periods: Vec<PeriodId>,
    pub partner_status: ShopPartnerStatusData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<Shop> for GetShopData {
    fn from(shop: Shop) -> Self {
        GetShopData {
            shop_id: shop.shop_id,
            shop_slug_id: shop.shop_slug_id,
            name: shop.name,
            shop_type: shop.shop_type.into(),
            domains: shop.domains,
            image: shop.image,
            structured_address: shop.structured_address,
            geo_address: shop.geo_address,
            phone: shop.phone,
            email: shop.email,
            specialities_categories: shop.specialities_categories,
            specialities_periods: shop.specialities_periods,
            partner_status: shop.partner_status.into(),
            created: shop.created,
            updated: shop.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{
        get_shop_data::GetShopData, partner_status_data::ShopPartnerStatusData,
        shop_type_data::ShopTypeData,
    };
    use common::{domain::Domain, shop_id::ShopId};
    use serde_json::json;
    use time::macros::datetime;
    use url::Url;

    #[test]
    fn should_serialize() {
        let datum = GetShopData {
            shop_id: ShopId::new(),
            shop_slug_id: "Woaah & Co. Ltd.".into(),
            name: "Woaah & Co. Ltd.".into(),
            shop_type: ShopTypeData::CommercialDealer,
            domains: [Domain::try_from("https://woaah.co.ltd.com").unwrap()].into(),
            image: Some(Url::parse("https://woaah.co.ltd.com/logo.svg").unwrap()),
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            specialities_categories: Vec::new(),
            specialities_periods: Vec::new(),
            partner_status: ShopPartnerStatusData::Partnered,
            created: datetime!(1976 - 12 - 01 0:00 UTC),
            updated: datetime!(1976 - 12 - 01 0:00 UTC),
        };

        let expected = json!({
            "shopId": datum.shop_id.to_string(),
            "shopSlugId": "woaah-co-ltd",
            "name": "Woaah & Co. Ltd.",
            "shopType": "COMMERCIAL_DEALER",
            "domains": ["woaah.co.ltd.com"],
            "image": "https://woaah.co.ltd.com/logo.svg",
            "partnerStatus": "PARTNERED",
            "created": "1976-12-01T00:00:00Z",
            "updated": "1976-12-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}
