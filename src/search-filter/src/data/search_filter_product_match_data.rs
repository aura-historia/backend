use crate::core::search_filter_product_match::SearchFilterProductMatch;
use common::actor::data::ActorData;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterProductMatchData {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub product_id: ProductId,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<bool>,
    pub created_by: ActorData,
    pub updated_by: ActorData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<SearchFilterProductMatch> for SearchFilterProductMatchData {
    fn from(product_match: SearchFilterProductMatch) -> Self {
        SearchFilterProductMatchData {
            user_id: product_match.user_id,
            user_search_filter_id: product_match.user_search_filter_id,
            shop_id: product_match.shop_id,
            shops_product_id: product_match.shops_product_id,
            product_id: product_match.product_id,
            feedback: product_match.feedback,
            created_by: product_match.created_by.into(),
            updated_by: product_match.updated_by.into(),
            created: product_match.created,
            updated: product_match.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for SearchFilterProductMatchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            SearchFilterProductMatchData {
                user_id: config.fake_with_rng(rng),
                user_search_filter_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                product_id: config.fake_with_rng(rng),
                feedback: config.fake_with_rng(rng),
                created_by: config.fake_with_rng(rng),
                updated_by: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
