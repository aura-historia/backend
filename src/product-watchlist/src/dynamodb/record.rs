use crate::core::watchlist_product::WatchlistProduct;
use crate::dynamodb::watchlist_product_state_record::WatchlistProductStateRecord;
use common::{
    product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId,
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct WatchlistProductRecord {
    pub pk: String,

    pub sk: String,

    pub lsi1_sk: String,

    pub gsi1_pk: String,

    pub gsi1_sk: String,

    pub user_id: UserId,

    pub product_id: ProductId,

    pub shop_id: ShopId,

    pub shops_product_id: ShopsProductId,

    pub notifications: bool,

    #[serde(default)]
    pub state: WatchlistProductStateRecord,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#watch#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_lsi1_sk(created: &OffsetDateTime) -> String {
    format!(
        "product#watch#created#{:020}",
        created.unix_timestamp_nanos()
    )
}

pub fn mk_gsi1_pk(product_id: &ProductId) -> String {
    format!("product_id#{product_id}")
}

pub fn mk_gsi1_sk(user_id: &UserId) -> String {
    format!("watch#user#{user_id}")
}

impl From<WatchlistProductRecord> for WatchlistProduct {
    fn from(record: WatchlistProductRecord) -> Self {
        WatchlistProduct {
            user_id: record.user_id,
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id,
            product_id: record.product_id,
            notifications: record.notifications,
            state: record.state.into(),
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for WatchlistProductRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let created = OffsetDateTime::now_utc();
            let product_id = config.fake_with_rng(rng);
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_product_id: ShopsProductId = config.fake_with_rng(rng);
            let user_id = config.fake_with_rng(rng);
            let notifications = config.fake_with_rng(rng);

            WatchlistProductRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&shop_id, &shops_product_id),
                lsi1_sk: mk_lsi1_sk(&created),
                gsi1_pk: mk_gsi1_pk(&product_id),
                gsi1_sk: mk_gsi1_sk(&user_id),
                user_id,
                product_id: config.fake_with_rng(rng),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                notifications,
                state: WatchlistProductStateRecord::Active,
                created,
                updated: created,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::record::WatchlistProductRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_watchlist_product_record() {
            let _ = Faker.fake::<WatchlistProductRecord>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamodb::{
        record::WatchlistProductRecord, record_update::WatchlistProductRecordUpdate,
    };

    #[test]
    fn should_be_subset_of_watchlist_record() {
        assert!(
            WatchlistProductRecordUpdate::SERDE_FIELDS
                .iter()
                .all(|field| WatchlistProductRecord::SERDE_FIELDS.contains(field))
        )
    }
}
