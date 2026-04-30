use crate::core::search_filter_product_match::SearchFilterProductMatch;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::event_id::EventId;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSearchFilterMatchRecord {
    pub pk: String,
    pub sk: String,
    pub lsi1_sk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsi2_sk: Option<String>,
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_search_filter_name: Option<String>,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub product_id: ProductId,
    pub origin_event_id: EventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_match_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<bool>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(
    search_filter_id: &UserSearchFilterId,
    shop_id: &ShopId,
    shops_product_id: &ShopsProductId,
) -> String {
    format!(
        "search_filter_match#search_filter#{search_filter_id}#shop_id#{shop_id}#shops_product_id#{shops_product_id}"
    )
}

pub fn mk_sk_prefix_filter(search_filter_id: &UserSearchFilterId) -> String {
    format!("search_filter_match#search_filter#{search_filter_id}#")
}

pub fn mk_sk_prefix_all() -> &'static str {
    "search_filter_match#"
}

pub fn mk_lsi1_sk(created: &OffsetDateTime) -> String {
    format!("search_filter_match#{:020}", created.unix_timestamp_nanos())
}

pub fn mk_lsi2_sk(
    shop_id: &ShopId,
    shops_product_id: &ShopsProductId,
    created: &OffsetDateTime,
) -> String {
    format!(
        "search_filter_match#shop_id#{shop_id}#shops_product_id#{shops_product_id}#{:020}",
        created.unix_timestamp_nanos()
    )
}

pub fn mk_lsi2_sk_prefix_product(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("search_filter_match#shop_id#{shop_id}#shops_product_id#{shops_product_id}#")
}

/// Lower bound for the `lsi1_sk` of all search filter match records.
pub const LSI1_SK_LOWER_BOUND: &str = "search_filter_match#";
/// Upper bound for the `lsi1_sk` of all search filter match records.
pub const LSI1_SK_UPPER_BOUND: &str = "search_filter_match#\u{ffff}";

impl From<UserSearchFilterMatchRecord> for SearchFilterProductMatch {
    fn from(record: UserSearchFilterMatchRecord) -> Self {
        SearchFilterProductMatch {
            user_id: record.user_id,
            user_search_filter_id: record.user_search_filter_id,
            user_search_filter_name: record
                .user_search_filter_name
                .map(UserSearchFilterName::from),
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id,
            product_id: record.product_id,
            origin_event_id: record.origin_event_id,
            enhanced_match_reason: record.enhanced_match_reason.map(EnhancedMatchReason::from),
            feedback: record.feedback,
            created: record.created,
            updated: record.updated,
        }
    }
}

impl From<SearchFilterProductMatch> for UserSearchFilterMatchRecord {
    fn from(m: SearchFilterProductMatch) -> Self {
        UserSearchFilterMatchRecord {
            pk: mk_pk(&m.user_id),
            sk: mk_sk(&m.user_search_filter_id, &m.shop_id, &m.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&m.created),
            lsi2_sk: Some(mk_lsi2_sk(&m.shop_id, &m.shops_product_id, &m.created)),
            user_id: m.user_id,
            user_search_filter_id: m.user_search_filter_id,
            user_search_filter_name: m.user_search_filter_name.map(Into::into),
            shop_id: m.shop_id,
            shops_product_id: m.shops_product_id,
            product_id: m.product_id,
            origin_event_id: m.origin_event_id,
            enhanced_match_reason: m.enhanced_match_reason.map(Into::into),
            feedback: m.feedback,
            created: m.created,
            updated: m.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use ::fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserSearchFilterMatchRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id: UserId = config.fake_with_rng(rng);
            let search_filter_id: UserSearchFilterId = config.fake_with_rng(rng);
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_product_id: ShopsProductId = config.fake_with_rng(rng);
            let created = OffsetDateTime::now_utc();
            UserSearchFilterMatchRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&search_filter_id, &shop_id, &shops_product_id),
                lsi1_sk: mk_lsi1_sk(&created),
                lsi2_sk: Some(mk_lsi2_sk(&shop_id, &shops_product_id, &created)),
                user_id,
                user_search_filter_id: search_filter_id,
                user_search_filter_name: config.fake_with_rng::<Option<String>, _>(rng),
                shop_id,
                shops_product_id,
                product_id: config.fake_with_rng(rng),
                origin_event_id: config.fake_with_rng(rng),
                enhanced_match_reason: config.fake_with_rng(rng),
                feedback: config.fake_with_rng(rng),
                created,
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ::fake::{Fake, Faker};

        #[test]
        fn should_fake_user_search_filter_match_record() {
            let _ = Faker.fake::<UserSearchFilterMatchRecord>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_sk_correctly() {
        let filter_id = UserSearchFilterId::new();
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
        assert!(sk.starts_with("search_filter_match#search_filter#"));
        assert!(sk.contains("#shop_id#"));
        assert!(sk.contains("#shops_product_id#"));
    }

    #[test]
    fn should_format_pk_correctly() {
        let user_id = UserId::new();
        let pk = mk_pk(&user_id);
        assert_eq!(pk, format!("user#{user_id}"));
    }

    #[test]
    fn should_format_lsi1_sk_correctly() {
        let created = OffsetDateTime::now_utc();
        let lsi1_sk = mk_lsi1_sk(&created);
        assert!(lsi1_sk.starts_with("search_filter_match#"));
        // 20-digit zero-padded nanosecond timestamp for stable lexicographic ordering
        let suffix = lsi1_sk.strip_prefix("search_filter_match#").unwrap();
        assert_eq!(20, suffix.len());
        assert!(suffix.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn should_format_lsi2_sk_correctly() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let created = OffsetDateTime::now_utc();
        let lsi2_sk = mk_lsi2_sk(&shop_id, &shops_product_id, &created);
        let expected_prefix =
            format!("search_filter_match#shop_id#{shop_id}#shops_product_id#{shops_product_id}#");
        assert!(lsi2_sk.starts_with(&expected_prefix));
        let suffix = lsi2_sk.strip_prefix(&expected_prefix).unwrap();
        assert_eq!(20, suffix.len());
        assert!(suffix.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn should_format_lsi2_sk_prefix_product_correctly() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let prefix = mk_lsi2_sk_prefix_product(&shop_id, &shops_product_id);
        assert_eq!(
            prefix,
            format!("search_filter_match#shop_id#{shop_id}#shops_product_id#{shops_product_id}#")
        );
    }
}
