use crate::{
    watchlist::dynamodb::record::{mk_gsi1_pk, mk_gsi1_sk},
    watchlist::service::command::UpdateWatchlistItemCommand,
};
use aws_sdk_dynamodb::{
    error::SdkError, operation::update_item::UpdateItemError, types::AttributeValue,
};
use common::{
    dynamodb_update::{DynamoDbUpdate, DynamoDbUpdateExpression},
    item_id::ItemId,
    user_id::UserId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_pk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_sk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for WatchlistItemRecordUpdate {
    #[allow(clippy::result_large_err)]
    fn into_update_expr(self) -> Result<DynamoDbUpdateExpression, SdkError<UpdateItemError>> {
        let mut remove_clause = "";
        if let Some(false) = self.notifications {
            remove_clause = "REMOVE gsi1_pk, gsi1_sk";
        }

        let mut update_expressions = Vec::new();
        let mut expr_attr_names = HashMap::new();
        let mut expr_attr_values = HashMap::new();

        let updates: HashMap<String, AttributeValue> =
            serde_dynamo::to_item(self).map_err(SdkError::construction_failure)?;
        let cleared_updates: HashMap<String, AttributeValue> =
            updates.into_iter().filter(|(_, v)| !v.is_null()).collect();
        for (attr, val) in cleared_updates {
            let attr_placeholder = format!("#{attr}");
            let val_placeholder = format!(":{attr}_val");

            update_expressions.push(format!("{attr_placeholder} = {val_placeholder}"));
            expr_attr_names.insert(attr_placeholder, attr);
            expr_attr_values.insert(val_placeholder, val);
        }

        if update_expressions.is_empty() {
            return Err(SdkError::construction_failure(
                "DynamoDb Update-Expression cannot be empty.",
            ));
        }

        let update_expr = DynamoDbUpdateExpression {
            update_expr: format!("SET {} {}", update_expressions.join(", "), remove_clause),
            expr_attr_names,
            expr_attr_values,
        };

        Ok(update_expr)
    }
}

impl WatchlistItemRecordUpdate {
    pub fn from_cmd(
        cmd: UpdateWatchlistItemCommand,
        user_id: &UserId,
        item_id: &ItemId,
    ) -> WatchlistItemRecordUpdate {
        WatchlistItemRecordUpdate {
            gsi1_pk: if let Some(true) = cmd.notifications {
                Some(mk_gsi1_pk(item_id))
            } else {
                None
            },
            gsi1_sk: if let Some(true) = cmd.notifications {
                Some(mk_gsi1_sk(user_id))
            } else {
                None
            },
            notifications: cmd.notifications,
            updated: OffsetDateTime::now_utc(),
        }
    }
}
