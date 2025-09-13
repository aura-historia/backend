use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::{
    error::SdkError, operation::update_item::UpdateItemError, types::AttributeValue,
};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct DynamoDbUpdateExpression {
    pub update_expr: String,
    pub expr_attr_names: HashMap<String, String>,
    pub expr_attr_values: HashMap<String, AttributeValue>,
}

#[allow(clippy::result_large_err)]
pub fn mk_update<T: Serialize>(
    t: T,
) -> Result<DynamoDbUpdateExpression, SdkError<UpdateItemError, HttpResponse>> {
    let mut update_expressions = Vec::new();
    let mut expr_attr_names = HashMap::new();
    let mut expr_attr_values = HashMap::new();

    let updates: HashMap<String, AttributeValue> =
        serde_dynamo::to_item(t).map_err(SdkError::construction_failure)?;
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
        update_expr: format!("SET {}", update_expressions.join(", ")),
        expr_attr_names,
        expr_attr_values,
    };

    Ok(update_expr)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::dynamodb_update::{DynamoDbUpdateExpression, mk_update};
    use aws_sdk_dynamodb::types::AttributeValue::*;
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    struct Dummy {
        #[serde(rename = "foo", default, skip_serializing_if = "Option::is_none")]
        f_oo: Option<String>,
        bar: Option<u64>,
    }

    #[rstest::rstest]
    #[case(
        Dummy { f_oo: Some("boop".into()), bar: None },
        DynamoDbUpdateExpression {
            update_expr: "SET #foo = :foo_val".into(),
            expr_attr_names: [("#foo".into(), "foo".into())].into(),
            expr_attr_values: [(":foo_val".into(), S("boop".into()))].into()
        }
    )]
    #[case(
        Dummy { f_oo: None, bar: Some(42) },
        DynamoDbUpdateExpression {
            update_expr: "SET #bar = :bar_val".into(),
            expr_attr_names: [("#bar".into(), "bar".into())].into(),
            expr_attr_values: [(":bar_val".into(), N("42".into()))].into()
        }
    )]
    fn should_mk_single_update_expr(
        #[case] dummy: Dummy,
        #[case] expected: DynamoDbUpdateExpression,
    ) {
        let actual = mk_update(dummy).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_mk_multipley_expr() {
        let dummy = Dummy {
            f_oo: Some("boop".into()),
            bar: Some(42),
        };

        let actual = mk_update(dummy).unwrap();

        assert!(
            "SET #foo = :foo_val, #bar = :bar_val" == actual.update_expr
                || "SET #bar = :bar_val, #foo = :foo_val" == actual.update_expr
        );
        assert_eq!(
            HashMap::from_iter([("#foo".into(), "foo".into()), ("#bar".into(), "bar".into())]),
            actual.expr_attr_names
        );
        assert_eq!(
            HashMap::from_iter([
                (":foo_val".into(), S("boop".into())),
                (":bar_val".into(), N("42".into())),
            ]),
            actual.expr_attr_values
        );
    }
}
