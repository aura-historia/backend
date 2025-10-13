use common::{item_id::ItemId, shop_id::ShopId, shops_item_id::ShopsItemId, user_id::UserId};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, error::Format, format_description::well_known::Rfc3339};

use crate::domain::WatchlistItem;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecord {
    pub pk: String,

    pub sk: String,

    pub user_id: UserId,

    pub item_id: ItemId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub notifications: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(created: &OffsetDateTime) -> Result<String, Format> {
    Ok(format!("item#watch#created#{}", created.format(&Rfc3339)?))
}

impl From<WatchlistItemRecord> for WatchlistItem {
    fn from(record: WatchlistItemRecord) -> Self {
        WatchlistItem {
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id,
            item_id: record.item_id,
            notifications: record.notifications,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for WatchlistItemRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let created = OffsetDateTime::now_utc();
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_item_id: ShopsItemId = config.fake_with_rng(rng);
            let user_id = Faker.fake::<UserId>();

            WatchlistItemRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&created).unwrap(),
                user_id,
                item_id: config.fake_with_rng(rng),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                notifications: config.fake_with_rng(rng),
                created,
                updated: created,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::record::WatchlistItemRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_watchlist_item_record() {
            let _ = Faker.fake::<WatchlistItemRecord>();
        }
    }
}
