use common::{shop_id::ShopId, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop_core::shop::Shop;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopRecord {
    pub pk: String,
    pub sk: String,
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

pub fn mk_pk(shop_id: &ShopId) -> String {
    format!("shop#{shop_id}")
}

impl From<Shop> for ShopRecord {
    fn from(shop: Shop) -> Self {
        ShopRecord {
            pk: mk_pk(&shop.shop_id),
            sk: "shop#details".to_owned(),
            shop_id: shop.shop_id,
            name: shop.name,
            url: shop.url,
            image: shop.image,
            created: shop.created,
            updated: shop.updated,
        }
    }
}

impl From<ShopRecord> for Shop {
    fn from(document: ShopRecord) -> Self {
        Shop {
            shop_id: document.shop_id,
            name: document.name,
            url: document.url,
            image: document.image,
            created: document.created,
            updated: document.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ShopRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Shop, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::shop_record::ShopRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_record() {
            let _ = Faker.fake::<ShopRecord>();
        }
    }
}
