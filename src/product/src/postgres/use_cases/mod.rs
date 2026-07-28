#![allow(dead_code)]

pub(crate) mod create_product;
pub(crate) mod delete_product;
pub(crate) mod update_product;

fn actor_label(context: &common::operation_context::OperationContext) -> Option<String> {
    match &context.principal {
        common::operation_context::Principal::Anonymous => None,
        common::operation_context::Principal::User(user_id) => Some(user_id.to_string()),
        common::operation_context::Principal::Service(service_id) => Some(service_id.clone()),
        common::operation_context::Principal::System => Some("SYSTEM".to_owned()),
    }
}
