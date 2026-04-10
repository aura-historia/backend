use crate::core::partner_shop_api_key::HashedPartnerShopApiKey;
use crate::core::{partner_shop::PartnerShop, shop::Shop};
use crate::dynamodb::shop_type_record::ShopTypeRecord;
use common::error::missing_field::MissingPersistenceField;
use common::{
    domain::Domain, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId, user_id::UserId,
};
use serde::{Deserialize, Serialize};
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
    pub shop_id: ShopId,
    pub shop_slug_id: SlugId<0>,
    pub name: ShopName,
    pub shop_type: ShopTypeRecord,

    #[serde(skip_serializing_if = "HashSet::is_empty", default)]
    pub domains: HashSet<Domain>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_api_key_short: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_api_key_long_hash: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_user_id: Option<UserId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi1_pk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi1_sk: Option<String>,

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

pub fn mk_gsi2_pk(shop_slug_id: &SlugId<0>) -> String {
    format!("shop_slug_id#{shop_slug_id}")
}

pub fn mk_gsi2_sk() -> &'static str {
    "shop#lookup#shop_id"
}

pub fn mk_gsi1_pk(partner_user_id: &UserId) -> String {
    format!("partner_user#{partner_user_id}")
}

pub fn mk_gsi1_sk(shop_id: &ShopId) -> String {
    format!("partner_shop_id#{shop_id}")
}

impl From<Shop> for ShopRecord {
    fn from(shop: Shop) -> Self {
        ShopRecord {
            pk: mk_pk(&shop.shop_id),
            sk: mk_sk().to_owned(),
            gsi2_pk: Some(mk_gsi2_pk(&shop.shop_slug_id)),
            gsi2_sk: Some(mk_gsi2_sk().to_owned()),
            shop_id: shop.shop_id,
            shop_slug_id: shop.shop_slug_id,
            name: shop.name,
            shop_type: shop.shop_type.into(),
            domains: shop.domains,
            image: shop.image,
            partner_api_key_short: None,
            partner_api_key_long_hash: None,
            partner_user_id: None,
            gsi1_pk: None,
            gsi1_sk: None,
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
            image: record.image,
            partner_status: if record.partner_api_key_short.is_some()
                && record.partner_api_key_long_hash.is_some()
            {
                crate::core::partner_status::ShopPartnerStatus::Partnered
            } else {
                crate::core::partner_status::ShopPartnerStatus::Scraped
            },
            created: record.created,
            updated: record.updated,
        }
    }
}

impl TryFrom<ShopRecord> for PartnerShop {
    type Error = MissingPersistenceField;

    fn try_from(value: ShopRecord) -> Result<Self, Self::Error> {
        let partner_api_key_short = value.partner_api_key_short.ok_or_else(|| {
            MissingPersistenceField::new(field::field!(partner_api_key_short@ShopRecord))
        })?;
        let partner_api_key_long_hash = value.partner_api_key_long_hash.ok_or_else(|| {
            MissingPersistenceField::new(field::field!(partner_api_key_long_hash@ShopRecord))
        })?;

        Ok(PartnerShop {
            shop_id: value.shop_id,
            shop_slug_id: value.shop_slug_id,
            name: value.name,
            shop_type: value.shop_type.into(),
            domains: value.domains,
            image: value.image,
            partner_user_id: value.partner_user_id,
            hashed_api_key: HashedPartnerShopApiKey::new(
                partner_api_key_short,
                partner_api_key_long_hash,
            ),
            created: value.created,
            updated: value.updated,
        })
    }
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

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn should_format_gsi1_pk_correctly() {
        let user_id = UserId::new();
        assert_eq!(mk_gsi1_pk(&user_id), format!("partner_user#{user_id}"));
    }

    #[test]
    fn should_format_gsi1_sk_correctly() {
        let shop_id = ShopId::new();
        assert_eq!(mk_gsi1_sk(&shop_id), format!("partner_shop_id#{shop_id}"));
    }
}
