use crate::dynamodb::partner_shop_application_record::PartnerShopApplicationStateRecord;
use common::dynamodb_update::DynamoDbUpdate;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct PartnerShopApplicationRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PartnerShopApplicationStateRecord>,

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
            updated: OffsetDateTime::now_utc(),
        };

        let expr = update.into_update_expr().unwrap();
        assert!(expr.update_expr.contains("SET"));
        assert!(expr.expr_attr_names.contains_key("#updated"));
        assert!(!expr.expr_attr_names.contains_key("#state"));
    }
}
