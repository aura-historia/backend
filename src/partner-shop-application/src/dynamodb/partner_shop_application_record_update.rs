use crate::dynamodb::partner_shop_application_state_record::PartnerShopApplicationStateRecord;
use common::execution_state::record::ExecutionStateRecord;
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
    pub business_state: Option<PartnerShopApplicationStateRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_state: Option<ExecutionStateRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name: Option<ShopName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_type: Option<ShopTypeRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_domains: Option<HashSet<Domain>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_image: Option<Url>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_token: Option<String>,

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
            business_state: Some(PartnerShopApplicationStateRecord::Approved),
            execution_state: None,
            shop_name: None,
            shop_type: None,
            shop_domains: None,
            shop_image: None,
            task_token: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#business_state"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
    }

    #[test]
    fn should_create_update_expr_with_only_updated() {
        let update = PartnerShopApplicationRecordUpdate {
            business_state: None,
            execution_state: None,
            shop_name: None,
            shop_type: None,
            shop_domains: None,
            shop_image: None,
            task_token: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
        assert!(!expr.expr_attr_names.contains_key("#business_state"));
    }

    #[test]
    fn should_create_update_expr_with_new_shop_name() {
        let update = PartnerShopApplicationRecordUpdate {
            business_state: None,
            execution_state: None,
            shop_name: Some(ShopName::from("Updated Shop".to_string())),
            shop_type: None,
            shop_domains: None,
            shop_image: None,
            task_token: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#shop_name"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
    }

    #[test]
    fn should_create_update_expr_with_all_shop_fields() {
        let update = PartnerShopApplicationRecordUpdate {
            business_state: Some(PartnerShopApplicationStateRecord::InReview),
            execution_state: Some(ExecutionStateRecord::Waiting),
            shop_name: Some(ShopName::from("Updated".to_string())),
            shop_type: Some(ShopTypeRecord::Marketplace),
            shop_domains: Some(HashSet::new()),
            shop_image: Some(Url::parse("https://example.com/image.png").unwrap()),
            task_token: None,
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#business_state"));
        assert!(expr.expr_attr_names.contains_key("#execution_state"));
        assert!(expr.expr_attr_names.contains_key("#shop_name"));
        assert!(expr.expr_attr_names.contains_key("#shop_type"));
        assert!(expr.expr_attr_names.contains_key("#shop_domains"));
        assert!(expr.expr_attr_names.contains_key("#shop_image"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
    }
}
