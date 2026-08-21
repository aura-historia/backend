use crate::{
    core::{
        address::{GeoAddress, StructuredAddress},
        continent::Continent,
        shop::Shop,
    },
    dynamodb::shop_record::ShopRecord,
    opensearch::{
        continent_document::ContinentDocument, partner_status_document::ShopPartnerStatusDocument,
        shop_type_document::ShopTypeDocument,
    },
};
use common::{
    actor::document::ActorDocument, domain::Domain, shop_id::ShopId, shop_name::ShopName,
    shop_slug_id::ShopSlugId,
};
use geo::opensearch::{geo_address_from_opensearch_point, geo_address_to_opensearch_point};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ShopDocument {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopTypeDocument,
    pub domains: HashSet<Domain>,

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
    pub structured_address_continent: Option<ContinentDocument>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,

    pub partner_status: ShopPartnerStatusDocument,
    pub created_by: ActorDocument,
    pub updated_by: ActorDocument,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ShopDocument {
    pub fn _id(&self) -> ShopId {
        self.shop_id
    }
}

impl From<Shop> for ShopDocument {
    fn from(shop: Shop) -> Self {
        ShopDocument {
            shop_id: shop.shop_id,
            shop_slug_id: shop.shop_slug_id,
            name: shop.name,
            shop_type: shop.shop_type.into(),
            domains: shop.domains,
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
            structured_address_continent: shop
                .structured_address
                .as_ref()
                .and_then(|a| a.continent)
                .map(ContinentDocument::from),
            geo_address: shop.geo_address.map(geo_address_to_opensearch_point),
            phone: shop.phone,
            email: shop.email,
            partner_status: shop.partner_status.into(),
            created_by: shop.created_by.into(),
            updated_by: shop.updated_by.into(),
            created: shop.created,
            updated: shop.updated,
        }
    }
}

impl From<ShopDocument> for Shop {
    fn from(document: ShopDocument) -> Self {
        Shop {
            shop_id: document.shop_id,
            shop_slug_id: document.shop_slug_id,
            name: document.name,
            shop_type: document.shop_type.into(),
            domains: document.domains,
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_webhook_secret: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: document.url,
            view_url: document.view_url,
            image: document.image,
            structured_address: structured_address_from_flat(
                document.structured_address_addressline,
                document.structured_address_addressline_extra,
                document.structured_address_locality,
                document.structured_address_region,
                document.structured_address_postal_code,
                document.structured_address_country,
            ),
            geo_address: document
                .geo_address
                .as_deref()
                .and_then(geo_address_from_opensearch_point),
            phone: document.phone,
            email: document.email,
            partner_status: document.partner_status.into(),
            affiliate_configuration: None,
            created_by: document.created_by.into(),
            updated_by: document.updated_by.into(),
            created: document.created,
            updated: document.updated,
        }
    }
}

impl From<ShopRecord> for ShopDocument {
    fn from(record: ShopRecord) -> Self {
        ShopDocument {
            shop_id: record.shop_id,
            shop_slug_id: record.shop_slug_id,
            name: record.name,
            shop_type: record.shop_type.into(),
            domains: record.domains,
            url: record.url,
            view_url: record.view_url,
            image: record.image,
            structured_address_addressline: record.structured_address_addressline,
            structured_address_addressline_extra: record.structured_address_addressline_extra,
            structured_address_locality: record.structured_address_locality,
            structured_address_region: record.structured_address_region,
            structured_address_postal_code: record.structured_address_postal_code,
            structured_address_country: record.structured_address_country,
            structured_address_continent: record
                .structured_address_country
                .map(|c| ContinentDocument::from(Continent::from(c))),
            geo_address: record
                .geo_address_lat
                .zip(record.geo_address_lon)
                .map(|(lat, lon)| geo_address_to_opensearch_point(GeoAddress { lat, lon })),
            phone: record.phone,
            email: record.email,
            partner_status: record.shop_partner_status.into(),
            created_by: record.created_by.into(),
            updated_by: record.updated_by.into(),
            created: record.created,
            updated: record.updated,
        }
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

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ShopDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Shop, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::shop_document::ShopDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_document() {
            let _ = Faker.fake::<ShopDocument>();
        }
    }
}
