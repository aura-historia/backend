use crate::{
    core::shop::Shop,
    data::{
        address_data::{GeoAddressData, StructuredAddressData},
        partner_status_data::ShopPartnerStatusData,
        shop_type_data::ShopTypeData,
    },
};
use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId};
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
    pub shopify_domain: Option<Domain>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_currency: Option<CurrencyData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_language: Option<LanguageData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_currency: Option<CurrencyData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_language: Option<LanguageData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<GeoAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,
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
            shopify_domain: shop.shopify_domain,
            shopify_currency: shop.shopify_currency.map(Into::into),
            shopify_language: shop.shopify_language.map(Into::into),
            woocommerce_currency: shop.woocommerce_currency.map(Into::into),
            woocommerce_language: shop.woocommerce_language.map(Into::into),
            url: shop.url,
            view_url: shop.view_url,
            image: shop.image,
            structured_address: shop.structured_address.map(Into::into),
            geo_address: shop.geo_address.map(Into::into),
            phone: shop.phone,
            email: shop.email,
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
            shopify_domain: Some(Domain::try_from("woaah.myshopify.com").unwrap()),
            shopify_currency: None,
            shopify_language: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: Some(Url::parse("https://woaah.co.ltd.com").unwrap()),
            view_url: Some(
                Url::parse(
                    "https://woaah.co.ltd.com/?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
            ),
            image: Some(Url::parse("https://woaah.co.ltd.com/logo.svg").unwrap()),
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
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
            "shopifyDomain": "woaah.myshopify.com",
            "url": "https://woaah.co.ltd.com/",
            "viewUrl": "https://woaah.co.ltd.com/?utm_source=aura_historia&utm_medium=referral",
            "image": "https://woaah.co.ltd.com/logo.svg",
            "partnerStatus": "PARTNERED",
            "created": "1976-12-01T00:00:00Z",
            "updated": "1976-12-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}
