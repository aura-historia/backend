use crate::{
    core::{
        first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier, user::User,
    },
    dynamodb::user_record::UserRecord,
    opensearch::{role_document::UserRoleDocument, tier_document::UserTierDocument},
};
use common::{
    currency::record::CurrencyRecord, language::record::LanguageRecord,
    stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use geo::core::address::{GeoAddress, StructuredAddress};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct UserDocument {
    pub user_id: UserId,
    pub email: Email,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyRecord>,
    pub prohibited_content_consent: bool,
    pub tier: UserTierDocument,
    pub role: UserRoleDocument,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_addressline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_addressline_extra: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_locality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_country: Option<CountryCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_address: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl UserDocument {
    pub fn _id(&self) -> UserId {
        self.user_id
    }
}

impl From<User> for UserDocument {
    fn from(user: User) -> Self {
        UserDocument {
            user_id: user.user_id,
            email: user.email,
            first_name: user.first_name,
            last_name: user.last_name,
            language: user.language.map(LanguageRecord::from),
            currency: user.currency.map(CurrencyRecord::from),
            prohibited_content_consent: user.prohibited_content_consent,
            tier: user.tier.into(),
            role: user.role.into(),
            stripe_customer_id: user.stripe_customer_id,
            structured_address_addressline: user
                .structured_address
                .as_ref()
                .and_then(|a| a.addressline.clone()),
            structured_address_addressline_extra: user
                .structured_address
                .as_ref()
                .and_then(|a| a.addressline_extra.clone()),
            structured_address_locality: user
                .structured_address
                .as_ref()
                .and_then(|a| a.locality.clone()),
            structured_address_region: user
                .structured_address
                .as_ref()
                .and_then(|a| a.region.clone()),
            structured_address_postal_code: user
                .structured_address
                .as_ref()
                .and_then(|a| a.postal_code.clone()),
            structured_address_country: user.structured_address.as_ref().and_then(|a| a.country),
            geo_address: user.geo_address.map(GeoAddress::to_opensearch_geo_point),
            created: user.created,
            updated: user.updated,
        }
    }
}

impl From<UserDocument> for User {
    fn from(document: UserDocument) -> Self {
        User {
            user_id: document.user_id,
            email: document.email,
            first_name: document.first_name,
            last_name: document.last_name,
            language: document.language.map(Into::into),
            currency: document.currency.map(Into::into),
            prohibited_content_consent: document.prohibited_content_consent,
            tier: UserTier::from(document.tier),
            role: UserRole::from(document.role),
            stripe_customer_id: document.stripe_customer_id,
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
                .and_then(GeoAddress::from_opensearch_geo_point),
            created: document.created,
            updated: document.updated,
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
    let structured_address = StructuredAddress {
        addressline,
        addressline_extra,
        locality,
        region,
        postal_code,
        country,
        continent: country.map(geo::core::continent::Continent::from),
    };
    (!structured_address.is_empty()).then_some(structured_address)
}

impl From<UserRecord> for UserDocument {
    fn from(record: UserRecord) -> Self {
        UserDocument::from(User::from(record))
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<User, _>(rng).into()
        }
    }
}
