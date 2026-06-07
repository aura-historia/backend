use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawShopNameRecord {
    pub pk: String,
    pub sk: String,
    pub raw_name: ShopName,

    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(raw_shop_name: &ShopName) -> String {
    format!("shop#raw_shop_name#{raw_shop_name}")
}

pub fn mk_sk() -> &'static str {
    "shop#lookup#raw_shop_name"
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for RawShopNameRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let raw_name = config.fake_with_rng::<ShopName, _>(rng);
            let shop_id = config.fake_with_rng::<ShopId, _>(rng);
            let name = config.fake_with_rng::<ShopName, _>(rng);
            let shop_slug_id = ShopSlugId::from(name.as_ref());
            let pk = mk_pk(&raw_name);
            let sk = mk_sk().to_owned();
            RawShopNameRecord {
                pk,
                sk,
                raw_name,
                shop_id,
                shop_slug_id,
                name,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::raw_shop_name_record::RawShopNameRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_raw_shop_name_record() {
            for _ in 0..100 {
                let _ = Faker.fake::<RawShopNameRecord>();
            }
        }
    }
}
