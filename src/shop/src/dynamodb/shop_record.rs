use crate::dynamodb::affiliate_configuration_record::AffiliateConfigurationRecord;
use crate::dynamodb::shop_type_record::ShopTypeRecord;
use crate::{
    core::{
        address::{GeoAddress, StructuredAddress},
        affiliate_configuration::AffiliateConfiguration,
        continent::Continent,
        partner_shop::PartnerShop,
        shop::Shop,
        woocommerce_webhook_secret::WoocommerceWebhookSecret,
    },
    dynamodb::partner_status_record::ShopPartnerStatusRecord,
};
use common::actor::record::ActorRecord;
use common::currency::record::CurrencyRecord;
use common::language::record::LanguageRecord;
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopRecord {
    pub pk: String,
    pub sk: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi2_pk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi2_sk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi3_pk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi3_sk: Option<String>,
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopTypeRecord,
    pub shop_partner_status: ShopPartnerStatusRecord,

    #[serde(skip_serializing_if = "HashSet::is_empty", default)]
    pub domains: HashSet<Domain>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_domain: Option<Domain>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_currency: Option<CurrencyRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_language: Option<LanguageRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_currency: Option<CurrencyRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_language: Option<LanguageRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline_extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_country: Option<CountryCode>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address_lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address_lon: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub affiliate_configuration: Option<AffiliateConfigurationRecord>,

    pub created_by: ActorRecord,
    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId) -> String {
    format!("shop#shop_id#{shop_id}")
}

pub fn mk_sk() -> &'static str {
    "shop#details"
}

pub fn mk_gsi2_pk(shop_slug_id: &ShopSlugId) -> String {
    format!("shop_slug_id#{shop_slug_id}")
}

pub fn mk_gsi2_sk() -> &'static str {
    "shop#lookup#shop_id"
}

pub fn mk_gsi3_pk(shopify_domain: &Domain) -> String {
    format!("shop#shopify_domain#{shopify_domain}")
}

pub fn mk_gsi3_sk() -> &'static str {
    "shop#details"
}

impl From<Shop> for ShopRecord {
    fn from(shop: Shop) -> Self {
        let (gsi3_pk, gsi3_sk) = shop
            .shopify_domain
            .as_ref()
            .map(|domain| (Some(mk_gsi3_pk(domain)), Some(mk_gsi3_sk().to_owned())))
            .unwrap_or((None, None));
        let affiliate_configuration = shop
            .affiliate_configuration
            .map(AffiliateConfigurationRecord::from);
        ShopRecord {
            pk: mk_pk(&shop.shop_id),
            sk: mk_sk().to_owned(),
            gsi3_pk,
            gsi3_sk,
            gsi2_pk: Some(mk_gsi2_pk(&shop.shop_slug_id)),
            gsi2_sk: Some(mk_gsi2_sk().to_owned()),
            shop_id: shop.shop_id,
            shop_slug_id: shop.shop_slug_id,
            name: shop.name,
            shop_type: shop.shop_type.into(),
            shop_partner_status: shop.partner_status.into(),
            domains: shop.domains,
            shopify_domain: shop.shopify_domain,
            shopify_currency: shop.shopify_currency.map(Into::into),
            shopify_language: shop.shopify_language.map(Into::into),
            woocommerce_webhook_secret: shop.woocommerce_webhook_secret,
            woocommerce_currency: shop.woocommerce_currency.map(Into::into),
            woocommerce_language: shop.woocommerce_language.map(Into::into),
            url: shop.url,
            view_url: shop.view_url,
            image: shop.image,
            structured_address_addressline: shop
                .structured_address
                .as_ref()
                .and_then(|a| a.addressline.clone()),
            structured_address_addressline_extra: shop
                .structured_address
                .as_ref()
                .and_then(|a| a.addressline_extra.clone()),
            structured_address_locality: shop
                .structured_address
                .as_ref()
                .and_then(|address| address.locality.clone()),
            structured_address_region: shop
                .structured_address
                .as_ref()
                .and_then(|address| address.region.clone()),
            structured_address_postal_code: shop
                .structured_address
                .as_ref()
                .and_then(|address| address.postal_code.clone()),
            structured_address_country: shop.structured_address.as_ref().and_then(|a| a.country),
            geo_address_lat: shop.geo_address.map(|address| address.lat),
            geo_address_lon: shop.geo_address.map(|address| address.lon),
            phone: shop.phone,
            email: shop.email,
            affiliate_configuration,
            created_by: shop.created_by.into(),
            updated_by: shop.updated_by.into(),
            created: shop.created,
            updated: shop.updated,
        }
    }
}

impl From<ShopRecord> for Shop {
    fn from(record: ShopRecord) -> Self {
        Shop {
            shop_id: record.shop_id,
            shop_slug_id: record.shop_slug_id,
            name: record.name,
            shop_type: record.shop_type.into(),
            domains: record.domains,
            shopify_domain: record.shopify_domain,
            shopify_currency: record.shopify_currency.map(Into::into),
            shopify_language: record.shopify_language.map(Into::into),
            woocommerce_webhook_secret: record.woocommerce_webhook_secret,
            woocommerce_currency: record.woocommerce_currency.map(Into::into),
            woocommerce_language: record.woocommerce_language.map(Into::into),
            url: record.url,
            view_url: record.view_url,
            image: record.image,
            structured_address: structured_address_from_flat(
                record.structured_address_addressline,
                record.structured_address_addressline_extra,
                record.structured_address_locality,
                record.structured_address_region,
                record.structured_address_postal_code,
                record.structured_address_country,
            ),
            geo_address: geo_address_from_flat(record.geo_address_lat, record.geo_address_lon),
            phone: record.phone,
            email: record.email,
            partner_status: record.shop_partner_status.into(),
            affiliate_configuration: record
                .affiliate_configuration
                .map(AffiliateConfiguration::from),
            created_by: record.created_by.into(),
            updated_by: record.updated_by.into(),
            created: record.created,
            updated: record.updated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("This shop is not a partner-shop.")]
pub struct NotAPartnerShopError;

impl TryFrom<ShopRecord> for PartnerShop {
    type Error = NotAPartnerShopError;

    fn try_from(value: ShopRecord) -> Result<Self, Self::Error> {
        if value.shop_partner_status != ShopPartnerStatusRecord::Partnered {
            return Err(NotAPartnerShopError);
        }
        Ok(PartnerShop {
            shop_id: value.shop_id,
            shop_slug_id: value.shop_slug_id,
            name: value.name,
            shop_type: value.shop_type.into(),
            domains: value.domains,
            shopify_domain: value.shopify_domain,
            shopify_currency: value.shopify_currency.map(Into::into),
            shopify_language: value.shopify_language.map(Into::into),
            woocommerce_webhook_secret: value.woocommerce_webhook_secret,
            woocommerce_currency: value.woocommerce_currency.map(Into::into),
            woocommerce_language: value.woocommerce_language.map(Into::into),
            url: value.url,
            view_url: value.view_url,
            image: value.image,
            structured_address: structured_address_from_flat(
                value.structured_address_addressline,
                value.structured_address_addressline_extra,
                value.structured_address_locality,
                value.structured_address_region,
                value.structured_address_postal_code,
                value.structured_address_country,
            ),
            geo_address: geo_address_from_flat(value.geo_address_lat, value.geo_address_lon),
            phone: value.phone,
            email: value.email,
            affiliate_configuration: value
                .affiliate_configuration
                .map(AffiliateConfiguration::from),
            created_by: value.created_by.into(),
            updated_by: value.updated_by.into(),
            created: value.created,
            updated: value.updated,
        })
    }
}

fn structured_address_from_flat(
    addressline: Option<String>,
    addressline_extra: Option<String>,
    locality: Option<String>,
    region: Option<String>,
    postal_code: Option<String>,
    country: Option<CountryCode>,
) -> Option<StructuredAddress> {
    let continent = country.map(Continent::from);
    let structured_address = StructuredAddress {
        addressline,
        addressline_extra,
        locality,
        region,
        postal_code,
        country,
        continent,
    };
    (!structured_address.is_empty()).then_some(structured_address)
}

fn geo_address_from_flat(lat: Option<f64>, lon: Option<f64>) -> Option<GeoAddress> {
    Some(GeoAddress {
        lat: lat?,
        lon: lon?,
    })
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ShopRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let shop = config.fake_with_rng::<Shop, _>(rng);
            ShopRecord::from(shop)
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::shop_record::ShopRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_record() {
            for _ in 0..100 {
                let _ = Faker.fake::<ShopRecord>();
            }
        }
    }
}

#[cfg(all(test, feature = "test-data"))]
mod utm_tests {
    use super::*;
    use crate::core::shop::Shop;
    use fake::{Fake, Faker};

    #[test]
    fn should_keep_raw_url_when_mapping_shop_record_to_shop() {
        let mut record = Faker.fake::<ShopRecord>();
        record.url = Some(Url::parse("https://example-shop.com").unwrap());

        let shop: Shop = record.into();

        assert_eq!(
            shop.url.as_ref().map(|u| u.as_str()),
            Some("https://example-shop.com/")
        );
    }

    #[test]
    fn should_return_none_url_when_shop_record_has_no_url() {
        let mut record = Faker.fake::<ShopRecord>();
        record.url = None;
        record.view_url = None;

        let shop: Shop = record.into();

        assert!(shop.url.is_none());
        assert!(shop.view_url.is_none());
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn should_format_gsi3_keys_correctly() {
        let shopify_domain = Domain::try_from("example.myshopify.com").unwrap();
        assert_eq!(
            mk_gsi3_pk(&shopify_domain),
            "shop#shopify_domain#example.myshopify.com"
        );
        assert_eq!(mk_gsi3_sk(), "shop#details");
    }
}
