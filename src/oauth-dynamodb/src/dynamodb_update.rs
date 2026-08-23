use aws_sdk_dynamodb::{
    config::http::HttpResponse, error::SdkError, operation::update_item::UpdateItemError,
    types::AttributeValue,
};
use serde::Serialize;
use std::collections::HashMap;

pub(crate) struct DynamoDbUpdateExpression {
    pub(crate) update_expr: String,
    pub(crate) expr_attr_names: HashMap<String, String>,
    pub(crate) expr_attr_values: HashMap<String, AttributeValue>,
}

pub(crate) trait DynamoDbUpdate: Serialize + Sized {
    #[allow(clippy::result_large_err)]
    fn into_update_expr(
        self,
    ) -> Result<DynamoDbUpdateExpression, SdkError<UpdateItemError, HttpResponse>> {
        let updates: HashMap<String, AttributeValue> =
            serde_dynamo::to_item(self).map_err(SdkError::construction_failure)?;
        let mut update_expressions = Vec::with_capacity(updates.len());
        let mut expr_attr_names = HashMap::with_capacity(updates.len());
        let mut expr_attr_values = HashMap::with_capacity(updates.len());

        for (attribute, value) in updates.into_iter().filter(|(_, value)| !value.is_null()) {
            let attribute_placeholder = format!("#{attribute}");
            let value_placeholder = format!(":{attribute}_val");
            update_expressions.push(format!("{attribute_placeholder} = {value_placeholder}"));
            expr_attr_names.insert(attribute_placeholder, attribute);
            expr_attr_values.insert(value_placeholder, value);
        }

        if update_expressions.is_empty() {
            return Err(SdkError::construction_failure(
                "DynamoDb Update-Expression cannot be empty.",
            ));
        }

        Ok(DynamoDbUpdateExpression {
            update_expr: format!("SET {}", update_expressions.join(", ")),
            expr_attr_names,
            expr_attr_values,
        })
    }
}
