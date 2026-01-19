use crate::{
    core::shop::Shop, dynamodb::shop_record::ShopRecord,
    opensearch::shop_type_document::ShopTypeDocument,
};
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ShopDocument {
    pub shop_id: ShopId,
    pub slug_id: SlugId<0>,
    pub name: ShopName,
    pub shop_type: ShopTypeDocument,
    pub domains: HashSet<Domain>,

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
            slug_id: shop.slug_id,
            name: shop.name,
            shop_type: shop.shop_type.into(),
            domains: shop.domains,
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
            slug_id: document.slug_id,
            name: document.name,
            shop_type: document.shop_type.into(),
            domains: document.domains,
            image: document.image,
            created: document.created,
            updated: document.updated,
        }
    }
}

impl From<ShopRecord> for ShopDocument {
    fn from(record: ShopRecord) -> Self {
        ShopDocument {
            shop_id: record.shop_id,
            slug_id: record.slug_id,
            name: record.name,
            shop_type: record.shop_type.into(),
            domains: record.domains,
            image: record.image,
            created: record.created,
            updated: record.updated,
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
        use crate::opensearch::shop_document::ShopDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_document() {
            let _ = Faker.fake::<ShopDocument>();
        }
    }
}
