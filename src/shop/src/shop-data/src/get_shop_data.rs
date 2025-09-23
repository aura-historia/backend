use common::{shop_id::ShopId, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop_core::shop::Shop;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetShopData {
    pub shop_id: ShopId,
    pub name: ShopName,
    pub url: Url,

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
            name: shop.name,
            url: shop.url,
            image: shop.image,
            created: shop.created,
            updated: shop.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::get_shop_data::GetShopData;
    use common::shop_id::ShopId;
    use serde_json::json;
    use time::macros::datetime;
    use url::Url;

    #[test]
    fn should_serialize() {
        let datum = GetShopData {
            shop_id: ShopId::new(),
            name: "Woaah & Co. Ltd.".into(),
            url: Url::parse("https://woaah.co.ltd.com").unwrap(),
            image: Some(Url::parse("https://woaah.co.ltd.com/logo.svg").unwrap()),
            created: datetime!(1976 - 12 - 01 0:00 UTC),
            updated: datetime!(1976 - 12 - 01 0:00 UTC),
        };

        let expected = json!({
            "shopId": datum.shop_id.to_string(),
            "name": "Woaah & Co. Ltd.",
            "url": "https://woaah.co.ltd.com/",
            "image": "https://woaah.co.ltd.com/logo.svg",
            "created": "1976-12-01T00:00:00Z",
            "updated": "1976-12-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}
