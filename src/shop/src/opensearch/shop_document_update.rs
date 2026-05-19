use crate::opensearch::{
    continent_document::ContinentDocument, partner_status_document::ShopPartnerStatusDocument,
    shop_type_document::ShopTypeDocument,
};
use common::domain::Domain;
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ShopDocumentUpdate {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_status: Option<ShopPartnerStatusDocument>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,
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

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl Default for ShopDocumentUpdate {
    fn default() -> Self {
        ShopDocumentUpdate {
            shop_type: None,
            partner_status: None,
            domains: None,
            url: None,
            view_url: None,
            image: None,
            structured_address_addressline: None,
            structured_address_addressline_extra: None,
            structured_address_locality: None,
            structured_address_region: None,
            structured_address_postal_code: None,
            structured_address_country: None,
            structured_address_continent: None,
            geo_address: None,
            phone: None,
            email: None,
            updated: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ShopDocumentUpdate {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ShopDocumentUpdate {
                domains: config.fake_with_rng(rng),
                url: config.fake_with_rng(rng),
                view_url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
                shop_type: config.fake_with_rng(rng),
                partner_status: config.fake_with_rng(rng),
                structured_address_addressline: config.fake_with_rng(rng),
                structured_address_addressline_extra: config.fake_with_rng(rng),
                structured_address_locality: config.fake_with_rng(rng),
                structured_address_region: config.fake_with_rng(rng),
                structured_address_postal_code: config.fake_with_rng(rng),
                structured_address_country: None,
                structured_address_continent: config.fake_with_rng(rng),
                geo_address: None,
                phone: None,
                email: None,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::shop_document_update::ShopDocumentUpdate;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_document_update() {
            let _ = Faker.fake::<ShopDocumentUpdate>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::opensearch::{
        shop_document::ShopDocument, shop_document_update::ShopDocumentUpdate,
    };

    #[test]
    fn should_be_subset_of_shop_document() {
        assert!(
            ShopDocumentUpdate::SERDE_FIELDS
                .iter()
                .all(|field| ShopDocument::SERDE_FIELDS.contains(field))
        )
    }
}
