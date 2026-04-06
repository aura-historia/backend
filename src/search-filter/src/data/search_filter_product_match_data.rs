use crate::core::search_filter_product_match::SearchFilterProductMatch;
use crate::core::user_search_filter_id::UserSearchFilterId;
use common::event_id::EventId;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
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
    pub origin_event_id: EventId,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_match_reason: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<SearchFilterProductMatch> for SearchFilterProductMatchData {
    fn from(m: SearchFilterProductMatch) -> Self {
        SearchFilterProductMatchData {
            user_id: m.user_id,
            user_search_filter_id: m.user_search_filter_id,
            shop_id: m.shop_id,
            shops_product_id: m.shops_product_id,
            product_id: m.product_id,
            origin_event_id: m.origin_event_id,
            enhanced_match_reason: m.enhanced_match_reason.map(Into::into),
            created: m.created,
            updated: m.updated,
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
                origin_event_id: config.fake_with_rng(rng),
                enhanced_match_reason: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::enhanced_match_reason::EnhancedMatchReason;
    use serde_json::json;

    #[test]
    fn should_serialize_with_reason() {
        let user_id = UserId::new();
        let filter_id = UserSearchFilterId::new();
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let product_id = ProductId::new();
        let event_id = EventId::new();
        let created = time::macros::datetime!(2025-01-01 0:00 UTC);

        let data = SearchFilterProductMatchData {
            user_id,
            user_search_filter_id: filter_id,
            shop_id,
            shops_product_id: shops_product_id.clone(),
            product_id,
            origin_event_id: event_id,
            enhanced_match_reason: Some("Matches golden cufflinks criteria.".into()),
            created,
            updated: created,
        };

        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(
            json["enhancedMatchReason"],
            "Matches golden cufflinks criteria."
        );
        assert_eq!(json["userId"], user_id.to_string());
    }

    #[test]
    fn should_serialize_without_reason() {
        let data = SearchFilterProductMatchData {
            user_id: UserId::new(),
            user_search_filter_id: UserSearchFilterId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            product_id: ProductId::new(),
            origin_event_id: EventId::new(),
            enhanced_match_reason: None,
            created: time::macros::datetime!(2025-01-01 0:00 UTC),
            updated: time::macros::datetime!(2025-01-01 0:00 UTC),
        };

        let json = serde_json::to_value(&data).unwrap();
        assert!(json.get("enhancedMatchReason").is_none());
    }

    #[test]
    fn should_deserialize_with_reason() {
        let user_id = UserId::new();
        let filter_id = UserSearchFilterId::new();
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let product_id = ProductId::new();
        let event_id = EventId::new();

        let json = json!({
            "userId": user_id.to_string(),
            "userSearchFilterId": filter_id.to_string(),
            "shopId": shop_id.to_string(),
            "shopsProductId": shops_product_id.to_string(),
            "productId": product_id.to_string(),
            "originEventId": event_id.to_string(),
            "enhancedMatchReason": "Product matches perfectly.",
            "created": "2025-01-01T00:00:00Z",
            "updated": "2025-01-01T00:00:00Z"
        });

        let data: SearchFilterProductMatchData = serde_json::from_value(json).unwrap();
        assert_eq!(
            data.enhanced_match_reason,
            Some("Product matches perfectly.".into())
        );
    }

    #[test]
    fn should_deserialize_without_reason() {
        let json = json!({
            "userId": UserId::new().to_string(),
            "userSearchFilterId": UserSearchFilterId::new().to_string(),
            "shopId": ShopId::new().to_string(),
            "shopsProductId": ShopsProductId::new().to_string(),
            "productId": ProductId::new().to_string(),
            "originEventId": EventId::new().to_string(),
            "created": "2025-01-01T00:00:00Z",
            "updated": "2025-01-01T00:00:00Z"
        });

        let data: SearchFilterProductMatchData = serde_json::from_value(json).unwrap();
        assert_eq!(data.enhanced_match_reason, None);
    }

    #[test]
    fn should_convert_from_domain_with_reason() {
        let product_match = SearchFilterProductMatch {
            user_id: UserId::new(),
            user_search_filter_id: UserSearchFilterId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            product_id: ProductId::new(),
            origin_event_id: EventId::new(),
            enhanced_match_reason: Some(EnhancedMatchReason::from("Matches criteria.")),
            created: time::macros::datetime!(2025-01-01 0:00 UTC),
            updated: time::macros::datetime!(2025-01-01 0:00 UTC),
        };

        let data = SearchFilterProductMatchData::from(product_match);
        assert_eq!(data.enhanced_match_reason, Some("Matches criteria.".into()));
    }

    #[test]
    fn should_convert_from_domain_without_reason() {
        let product_match = SearchFilterProductMatch {
            user_id: UserId::new(),
            user_search_filter_id: UserSearchFilterId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            product_id: ProductId::new(),
            origin_event_id: EventId::new(),
            enhanced_match_reason: None,
            created: time::macros::datetime!(2025-01-01 0:00 UTC),
            updated: time::macros::datetime!(2025-01-01 0:00 UTC),
        };

        let data = SearchFilterProductMatchData::from(product_match);
        assert_eq!(data.enhanced_match_reason, None);
    }
}
