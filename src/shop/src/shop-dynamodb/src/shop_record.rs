use common::{
    shop_id::{ShopId, ShopIdentifier},
    shop_name::ShopName,
};
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

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub urls: Vec<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(shop_identifier: &ShopIdentifier) -> String {
    match shop_identifier {
        ShopIdentifier::ShopId(shop_id) => mk_pk_as_shop_id(shop_id),
        ShopIdentifier::ShopUrl(url) => mk_pk_as_shop_url(url),
    }
}

pub fn mk_pk_as_shop_id(shop_id: &ShopId) -> String {
    format!("shop#shop_id#{shop_id}")
}

pub fn mk_pk_as_shop_url(url: &Url) -> String {
    format!("shop#url#{url}")
}

impl ShopRecord {
    pub fn from_shop_as_shop_id_record(shop: Shop) -> ShopRecord {
        ShopRecord {
            pk: mk_pk_as_shop_id(&shop.shop_id),
            sk: "shop#details".to_owned(),
            shop_id: shop.shop_id,
            name: shop.name,
            urls: shop.urls,
            image: shop.image,
            created: shop.created,
            updated: shop.updated,
        }
    }

    pub fn clone_from_shop_as_shop_url_records(shop: &Shop) -> Vec<ShopRecord> {
        shop.urls
            .iter()
            .map(|url| ShopRecord {
                pk: mk_pk_as_shop_url(url),
                sk: "shop#details".to_owned(),
                shop_id: shop.shop_id,
                name: shop.name.clone(),
                urls: shop.urls.clone(),
                image: shop.image.clone(),
                created: shop.created,
                updated: shop.updated,
            })
            .collect()
    }
}

impl From<ShopRecord> for Shop {
    fn from(document: ShopRecord) -> Self {
        Shop {
            shop_id: document.shop_id,
            name: document.name,
            urls: document.urls,
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
            let shop = config.fake_with_rng::<Shop, _>(rng);
            if config.fake_with_rng(rng) {
                ShopRecord::from_shop_as_shop_id_record(shop)
            } else {
                ShopRecord::clone_from_shop_as_shop_url_records(&shop)
                    .first()
                    .unwrap()
                    .clone()
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::shop_record::ShopRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_record() {
            for _ in 0..100 {
                let _ = Faker.fake::<ShopRecord>();
            }
        }
    }
}
