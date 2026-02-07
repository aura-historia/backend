use crate::watchlist::core::watchlist_product::WatchlistProduct;
use common::{
    product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId,
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::{OffsetDateTime, error::Format, format_description::well_known::Rfc3339};
use user::dynamodb::user_record::UserRecord;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct WatchlistProductRecord {
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

    pub product_id: ProductId,

    pub shop_id: ShopId,

    pub shops_product_id: ShopsProductId,

    pub notifications: bool,

    // see WatchlistProductDynamoDbRepositoryImpl::query_user_records_with_notifications
    pub user_record: UserRecord,

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

pub fn mk_lsi1_sk(created: &OffsetDateTime) -> Result<String, Format> {
    Ok(format!(
        "product#watch#created#{}",
        created.format(&Rfc3339)?
    ))
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
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id,
            product_id: record.product_id,
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

    impl Dummy<Faker> for WatchlistProductRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let created = OffsetDateTime::now_utc();
            let product_id = config.fake_with_rng(rng);
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_product_id: ShopsProductId = config.fake_with_rng(rng);
            let user_id = config.fake_with_rng(rng);
            let notifications = config.fake_with_rng(rng);

            WatchlistProductRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&shop_id, &shops_product_id),
                lsi1_sk: mk_lsi1_sk(&created).unwrap(),
                gsi1_pk: if notifications {
                    Some(mk_gsi1_pk(&product_id))
                } else {
                    None
                },
                gsi1_sk: if notifications {
                    Some(mk_gsi1_sk(&user_id))
                } else {
                    None
                },
                user_id,
                product_id: config.fake_with_rng(rng),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                notifications,
                user_record: UserRecord {
                    pk: user_record::mk_pk(&user_id),
                    sk: user_record::mk_sk().to_owned(),
                    user_id,
                    email: SafeEmail()
                        .fake_with_rng::<String, R>(rng)
                        .try_into()
                        .unwrap(),
                    first_name: config.fake_with_rng(rng),
                    last_name: config.fake_with_rng(rng),
                    language: config.fake_with_rng(rng),
                    currency: config.fake_with_rng(rng),
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
        use crate::watchlist::dynamodb::record::WatchlistProductRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_watchlist_product_record() {
            let _ = Faker.fake::<WatchlistProductRecord>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::watchlist::dynamodb::{
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
