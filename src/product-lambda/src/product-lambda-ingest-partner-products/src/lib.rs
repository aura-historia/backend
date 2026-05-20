pub mod service;
pub mod types;

pub use service::{
    AsyncProductCommandFailure, AsyncProductCommandService, AsyncProductCommandServiceImpl,
};
pub use types::{
    AsyncProductCommandData, CreateAsyncProductCommandData, UpdateAsyncProductCommandData,
    UpsertAsyncProductCommandData,
};

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::has_key::HasKey;
use common::product_id::ProductKey;
use lambda_runtime::LambdaEvent;
use product::service::command_service::CommandProductService;
use product::service::product_command::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use std::collections::{HashMap, HashSet};
use tracing::{error, info};

#[tracing::instrument(skip(event, product_service), fields(requestId = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    product_service: &(impl CommandProductService + Sync),
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let mut failed_message_ids = Vec::new();
    let mut creates: Vec<(String, CreateProductCommand)> = Vec::new();
    let mut updates: Vec<(String, ProductKey, UpdateProductCommand)> = Vec::new();
    let mut upserts: Vec<(String, UpsertProductCommand)> = Vec::new();

    for message in event.payload.records {
        let Some(message_id) = message.message_id else {
            continue;
        };
        let Some(body) = message.body else {
            continue;
        };
        match serde_json::from_str::<AsyncProductCommandData>(&body) {
            Ok(AsyncProductCommandData::Create(command)) => {
                creates.push((message_id, command.into()))
            }
            Ok(AsyncProductCommandData::Update(command)) => {
                let (key, command) = command.into();
                updates.push((message_id, key, command));
            }
            Ok(AsyncProductCommandData::Upsert(command)) => {
                upserts.push((message_id, command.into()))
            }
            Err(err) => {
                error!(messageId = %message_id, error = %err, "Failed deserializing async product command.");
                failed_message_ids.push(message_id);
            }
        }
    }

    if !creates.is_empty() {
        let commands = creates.iter().map(|(_, command)| command.clone()).collect();
        let failed_keys: HashSet<ProductKey> = product_service
            .create(commands)
            .await
            .into_iter()
            .map(|command| command.key())
            .collect();
        for (message_id, command) in &creates {
            if failed_keys.contains(&command.key()) {
                failed_message_ids.push(message_id.clone());
            }
        }
    }

    if !updates.is_empty() {
        let commands: HashMap<ProductKey, UpdateProductCommand> = updates
            .iter()
            .map(|(_, key, command)| (key.clone(), command.clone()))
            .collect();
        let failed_keys: HashSet<ProductKey> =
            product_service.update(commands).await.into_keys().collect();
        for (message_id, key, _) in &updates {
            if failed_keys.contains(key) {
                failed_message_ids.push(message_id.clone());
            }
        }
    }

    if !upserts.is_empty() {
        let commands = upserts.iter().map(|(_, command)| command.clone()).collect();
        let failed_keys: HashSet<ProductKey> = product_service
            .upsert(commands)
            .await
            .into_iter()
            .map(|command| command.key())
            .collect();
        for (message_id, command) in &upserts {
            if failed_keys.contains(&command.key()) {
                failed_message_ids.push(message_id.clone());
            }
        }
    }

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures,
        failures = failures,
        "Handler finished."
    );

    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(|item_identifier| {
            let mut failure = BatchItemFailure::default();
            failure.item_identifier = item_identifier;
            failure
        })
        .collect();
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use lambda_runtime::Context;
    use product::service::command_service::MockCommandProductService;

    fn upsert_command(id: &str) -> AsyncProductCommandData {
        AsyncProductCommandData::Upsert(UpsertAsyncProductCommandData {
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::from(id.to_string()),
            title: None,
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            seller_name: None,
            structured_address: None,
            geo_address: None,
        })
    }

    fn event(command: AsyncProductCommandData) -> LambdaEvent<SqsEvent> {
        let mut message = SqsMessage::default();
        message.message_id = Some("msg-1".to_string());
        message.body = Some(serde_json::to_string(&command).unwrap());
        let mut event = SqsEvent::default();
        event.records = vec![message];
        LambdaEvent::new(event, Context::default())
    }

    #[tokio::test]
    async fn should_upsert_product_when_command_is_valid_for_handler() {
        let mut service = MockCommandProductService::default();
        service.expect_upsert().return_once(|commands| {
            Box::pin(async move {
                assert_eq!(1, commands.len());
                vec![]
            })
        });

        let response = handler(event(upsert_command("product-1")), &service)
            .await
            .unwrap();

        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_report_partial_failure_when_command_fails_for_handler() {
        let mut service = MockCommandProductService::default();
        service
            .expect_upsert()
            .return_once(|commands| Box::pin(async move { commands }));

        let response = handler(event(upsert_command("product-1")), &service)
            .await
            .unwrap();

        assert_eq!(1, response.batch_item_failures.len());
        assert_eq!("msg-1", response.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_report_partial_failure_when_body_is_invalid_for_handler() {
        let mut message = SqsMessage::default();
        message.message_id = Some("msg-1".to_string());
        message.body = Some("not json".to_string());
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![message];
        let event = LambdaEvent::new(sqs_event, Context::default());
        let service = MockCommandProductService::default();

        let response = handler(event, &service).await.unwrap();

        assert_eq!(1, response.batch_item_failures.len());
    }
}
