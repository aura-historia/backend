use crate::{core::shop::Shop, data::shop_type_data::ShopTypeData};
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId};
use serde::{Deserialize, Serialize};
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
            created: shop.created,
            updated: shop.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{get_shop_data::GetShopData, shop_type_data::ShopTypeData};
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
            created: datetime!(1976 - 12 - 01 0:00 UTC),
            updated: datetime!(1976 - 12 - 01 0:00 UTC),
        };

        let expected = json!({
            "shopId": datum.shop_id.to_string(),
            "productSlugId": "woaah-co-ltd",
            "name": "Woaah & Co. Ltd.",
            "shopType": "COMMERCIAL_DEALER",
            "domains": ["woaah.co.ltd.com"],
            "image": "https://woaah.co.ltd.com/logo.svg",
            "created": "1976-12-01T00:00:00Z",
            "updated": "1976-12-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}
