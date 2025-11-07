use crate::watchlist::core::watchlist_item::WatchlistItem;
use common::{item_id::ItemId, shop_id::ShopId, shops_item_id::ShopsItemId, user_id::UserId};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, error::Format, format_description::well_known::Rfc3339};
use user::dynamodb::user_record::UserRecord;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecord {
    pub pk: String,

    pub sk: String,

    pub lsi1_sk: String,

    // some if notifications, none else (conditional sparsity - only appears if notifications enabled
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_pk: Option<String>,

    // some if notifications, none else (conditional sparsity - only appears if notifications enabled
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_sk: Option<String>,

    pub user_id: UserId,

    pub item_id: ItemId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub notifications: bool,

    // see WatchlistItemDynamoDbRepositoryImpl::query_user_records_with_notifications
    pub user_record: UserRecord,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(shop_id: &ShopId, shops_item_id: &ShopsItemId) -> String {
    format!("item#watch#shop_id#{shop_id}#shops_item_id#{shops_item_id}")
}

pub fn mk_lsi1_sk(created: &OffsetDateTime) -> Result<String, Format> {
    Ok(format!("item#watch#created#{}", created.format(&Rfc3339)?))
}

pub fn mk_gsi1_pk(item_id: &ItemId) -> String {
    format!("item_id#{item_id}")
}

pub fn mk_gsi1_sk(user_id: &UserId) -> String {
    format!("user#{user_id}")
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
    use fake::{Dummy, Fake, Faker, Rng, faker::internet::de_de::SafeEmail};
    use user::dynamodb::user_record;

    impl Dummy<Faker> for WatchlistItemRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let created = OffsetDateTime::now_utc();
            let item_id = config.fake_with_rng(rng);
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_item_id: ShopsItemId = config.fake_with_rng(rng);
            let user_id = config.fake_with_rng(rng);
            let notifications = config.fake_with_rng(rng);

            WatchlistItemRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&shop_id, &shops_item_id),
                lsi1_sk: mk_lsi1_sk(&created).unwrap(),
                gsi1_pk: if notifications {
                    Some(mk_gsi1_pk(&item_id))
                } else {
                    None
                },
                gsi1_sk: if notifications {
                    Some(mk_gsi1_sk(&user_id))
                } else {
                    None
                },
                user_id,
                item_id: config.fake_with_rng(rng),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                notifications,
                user_record: UserRecord {
                    pk: user_record::mk_pk(&user_id),
                    sk: user_record::mk_sk().to_owned(),
                    id: user_id,
                    email: SafeEmail()
                        .fake_with_rng::<String, R>(rng)
                        .try_into()
                        .unwrap(),
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                },
                created,
                updated: created,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::watchlist::dynamodb::record::WatchlistItemRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_watchlist_item_record() {
            let _ = Faker.fake::<WatchlistItemRecord>();
        }
    }
}
