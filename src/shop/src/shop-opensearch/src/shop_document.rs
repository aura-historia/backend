use common::{shop_id::ShopId, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop_core::shop::Shop;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopDocument {
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

impl ShopDocument {
    pub fn _id(&self) -> ShopId {
        self.shop_id
    }
}

impl From<Shop> for ShopDocument {
    fn from(shop: Shop) -> Self {
        ShopDocument {
            shop_id: shop.shop_id,
            name: shop.name,
            url: shop.url,
            image: shop.image,
            created: shop.created,
            updated: shop.updated,
        }
    }
}

impl From<ShopDocument> for Shop {
    fn from(document: ShopDocument) -> Self {
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

    impl Dummy<Faker> for ShopDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Shop, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::shop_document::ShopDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_document() {
            let _ = Faker.fake::<ShopDocument>();
        }
    }
}
