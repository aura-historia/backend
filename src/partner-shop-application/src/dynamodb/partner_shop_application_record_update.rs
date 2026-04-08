use crate::dynamodb::partner_shop_application_state_record::PartnerShopApplicationStateRecord;
use common::{domain::Domain, dynamodb_update::DynamoDbUpdate, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct PartnerShopApplicationRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PartnerShopApplicationStateRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_shop_name: Option<ShopName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_shop_type: Option<ShopTypeRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_shop_domains: Option<HashSet<Domain>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_shop_image: Option<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for PartnerShopApplicationRecordUpdate {}

#[cfg(test)]
mod tests {
    use super::*;
    use common::dynamodb_update::DynamoDbUpdate;

    #[test]
    fn should_create_update_expr_with_state_and_updated() {
        let update = PartnerShopApplicationRecordUpdate {
            state: Some(PartnerShopApplicationStateRecord::Approved),
            new_shop_name: None,
            new_shop_type: None,
            new_shop_domains: None,
            new_shop_image: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#state"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
    }

    #[test]
    fn should_create_update_expr_with_only_updated() {
        let update = PartnerShopApplicationRecordUpdate {
            state: None,
            new_shop_name: None,
            new_shop_type: None,
            new_shop_domains: None,
            new_shop_image: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
        assert!(!expr.expr_attr_names.contains_key("#state"));
    }

    #[test]
    fn should_create_update_expr_with_new_shop_name() {
        let update = PartnerShopApplicationRecordUpdate {
            state: None,
            new_shop_name: Some(ShopName::from("Updated Shop".to_string())),
            new_shop_type: None,
            new_shop_domains: None,
            new_shop_image: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#new_shop_name"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
    }

    #[test]
    fn should_create_update_expr_with_all_shop_fields() {
        let update = PartnerShopApplicationRecordUpdate {
            state: Some(PartnerShopApplicationStateRecord::InReview),
            new_shop_name: Some(ShopName::from("Updated".to_string())),
            new_shop_type: Some(ShopTypeRecord::Marketplace),
            new_shop_domains: Some(HashSet::new()),
            new_shop_image: Some(Url::parse("https://example.com/image.png").unwrap()),
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#state"));
        assert!(expr.expr_attr_names.contains_key("#new_shop_name"));
        assert!(expr.expr_attr_names.contains_key("#new_shop_type"));
        assert!(expr.expr_attr_names.contains_key("#new_shop_domains"));
        assert!(expr.expr_attr_names.contains_key("#new_shop_image"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
    }
}
