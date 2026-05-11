mod types;

pub use types::{
    ShopifyEventDetail, ShopifyEventMetadata, ShopifyImagePayload, ShopifyProductEvent,
    ShopifyProductEventError, ShopifyProductEventKind, ShopifyProductPayload,
    ShopifyVariantPayload, html_to_text, infer_language, parse_price, product_state,
};

use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::domain::Domain;
use common::has_key::HasKey;
use common::product_id::ProductKey;
use lambda_runtime::LambdaEvent;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use serde_json::Value;
use shop::core::partner_status::ShopPartnerStatus;
use shop::service::get_service::{GetShopError, GetShopService};
use std::collections::HashMap;
use tracing::{error, info, warn};

pub const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
pub const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
pub const SHOPIFY_TOPIC_PRODUCTS_DELETE: &str = "products/delete";

/// Resolves a single EventBridge event to an `UpsertProductCommand`.
///
/// Returns:
/// - `Ok(Some(cmd))` — valid command ready for batch upsert.
/// - `Ok(None)` — ignorable event (unknown shop, unsupported topic, etc.).
/// - `Err(msg)` — transient error; the corresponding SQS message should be retried.
#[tracing::instrument(
    skip(event, shop_service),
    fields(
        eventBridgeEventId = tracing::field::Empty,
        shopifyEventId = tracing::field::Empty,
        shopifyTopic = tracing::field::Empty,
        shopifyDomain = tracing::field::Empty,
    )
)]
async fn resolve_command(
    event: EventBridgeEvent<Value>,
    shop_service: &(impl GetShopService + Sync),
) -> Result<Option<UpsertProductCommand>, String> {
    let span = tracing::Span::current();
    if let Some(event_bridge_event_id) = event.id.as_deref() {
        span.record("eventBridgeEventId", event_bridge_event_id);
    }

    let detail = match serde_json::from_value::<ShopifyEventDetail>(event.detail) {
        Ok(detail) => detail,
        Err(err) => {
            error!(error = %err, "Failed deserializing Shopify EventBridge detail.");
            return Ok(None);
        }
    };

    if let Some(event_id) = detail.metadata.event_id.as_deref() {
        span.record("shopifyEventId", event_id);
    }
    span.record("shopifyTopic", detail.metadata.topic.as_str());
    span.record("shopifyDomain", detail.metadata.shop_domain.as_str());

    let kind = match detail.metadata.topic.as_str() {
        SHOPIFY_TOPIC_PRODUCTS_CREATE => ShopifyProductEventKind::Create,
        SHOPIFY_TOPIC_PRODUCTS_UPDATE => ShopifyProductEventKind::Update,
        SHOPIFY_TOPIC_PRODUCTS_DELETE => ShopifyProductEventKind::Delete,
        other => {
            warn!(shopifyTopic = %other, "Received unsupported Shopify topic, ignoring.");
            return Ok(None);
        }
    };

    let shop_domain = match Domain::try_from(detail.metadata.shop_domain.as_str()) {
        Ok(domain) => domain,
        Err(err) => {
            warn!(error = %err, domain = detail.metadata.shop_domain.as_str(), "Shopify event contains invalid shop domain, ignoring.");
            return Ok(None);
        }
    };

    let shop = match shop_service.find_shop_by_shopify_domain(&shop_domain).await {
        Ok(shop) => shop,
        Err(GetShopError::ShopifyDomainNotFound(_)) => {
            warn!(shopifyDomain = %shop_domain, "Shopify event references unknown shop, ignoring.");
            return Ok(None);
        }
        Err(err) => return Err(err.to_string()),
    };

    if shop.partner_status != ShopPartnerStatus::Partnered {
        warn!(shopId = %shop.shop_id, shopifyDomain = %shop_domain, "Shopify event references non-partner shop, ignoring.");
        return Ok(None);
    }

    let shopify_event = ShopifyProductEvent {
        shop_id: shop.shop_id,
        shop_domain,
        kind,
        payload: detail.payload,
    };
    let command = match UpsertProductCommand::try_from(shopify_event) {
        Ok(command) => command,
        Err(err) => {
            error!(error = %err, "Failed mapping Shopify product event.");
            return Ok(None);
        }
    };

    Ok(Some(command))
}

/// SQS batch handler. Each SQS message body is an EventBridge event JSON
/// envelope published by the Shopify event rule.
///
/// All valid commands from the batch are collected and passed to
/// `product_service.upsert()` in a single call. Returned failures are mapped
/// back to their originating message IDs and reported as partial batch failures
/// so only the failed messages are retried.
#[tracing::instrument(skip(event, shop_service, product_service), fields(requestId = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    shop_service: &(impl GetShopService + Sync),
    product_service: &(impl CommandProductService + Sync),
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let mut failed_message_ids: Vec<String> = Vec::new();
    // Tracks which message ID produced each command so failed commands can be
    // mapped back to their SQS message after the batch upsert.
    let mut message_id_by_key: HashMap<ProductKey, String> = HashMap::new();
    let mut commands: Vec<UpsertProductCommand> = Vec::new();

    for message in event.payload.records {
        let message_id = match message.message_id {
            Some(id) => id,
            None => {
                warn!("Received SQS message without message_id, skipping.");
                continue;
            }
        };

        let body = match message.body {
            Some(b) => b,
            None => {
                info!(messageId = %message_id, "Received empty SQS body, skipping.");
                continue;
            }
        };

        let eb_event = match serde_json::from_str::<EventBridgeEvent<Value>>(&body) {
            Ok(e) => e,
            Err(err) => {
                error!(messageId = %message_id, error = %err, "Failed deserializing SQS body as EventBridgeEvent.");
                failed_message_ids.push(message_id);
                continue;
            }
        };

        match resolve_command(eb_event, shop_service).await {
            Ok(Some(cmd)) => {
                if let Some(displaced_msg_id) = message_id_by_key.insert(cmd.key(), message_id) {
                    // Two messages in the same batch share the same ProductKey. The
                    // earlier message is displaced in the map; report it as failed so
                    // it is retried rather than silently dropped.
                    warn!(
                        messageId = %displaced_msg_id,
                        "Duplicate ProductKey in batch; earlier message displaced, reporting as failure."
                    );
                    failed_message_ids.push(displaced_msg_id);
                }
                commands.push(cmd);
            }
            Ok(None) => {}
            Err(err) => {
                warn!(messageId = %message_id, error = %err, "Failed processing Shopify event.");
                failed_message_ids.push(message_id);
            }
        }
    }

    if !commands.is_empty() {
        let failed_commands = product_service.upsert(commands).await;
        for failed_cmd in failed_commands {
            if let Some(msg_id) = message_id_by_key.remove(&failed_cmd.key()) {
                failed_message_ids.push(msg_id);
            } else {
                warn!(
                    "Failed command has no matching message ID in batch tracking map; failure may be underreported."
                );
            }
        }
    }

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures,
        failures = failures,
        "Handler finished.",
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
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::domain::Domain;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::service::command_service::MockCommandProductService;
    use serde_json::{Value, json};
    use shop::core::partner_status::ShopPartnerStatus;
    use shop::core::shop::Shop;
    use shop::service::get_service::MockGetShopService;

    fn shopify_detail_with_product_id(topic: &str, product_id: u64) -> Value {
        json!({
            "payload": {
                "id": product_id,
                "body_html": "<p>Hallo Test Beschreibung!</p>",
                "handle": "thomas-testprodukt",
                "title": "Thomas Testprodukt",
                "vendor": "partner vendor",
                "status": "active",
                "variants": [{"price": "420.00", "inventory_quantity": 2}],
                "images": [{"src": "https://cdn.shopify.com/product.jpg"}]
            },
            "metadata": {
                "X-Shopify-Topic": topic,
                "X-Shopify-Shop-Domain": "partner-shop.myshopify.com",
                "X-Shopify-Event-Id": "event-1"
            }
        })
    }

    fn make_eb_event_with_product_id(topic: &str, product_id: u64) -> EventBridgeEvent<Value> {
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = "shopifyWebhook".to_string();
        event.source = "aws.partner/shopify.com/test".to_string();
        event.detail = shopify_detail_with_product_id(topic, product_id);
        event
    }

    fn make_sqs_event(topic: &str) -> LambdaEvent<SqsEvent> {
        make_sqs_event_with_id(topic, "msg-1")
    }

    fn make_sqs_event_with_id(topic: &str, message_id: &str) -> LambdaEvent<SqsEvent> {
        make_sqs_event_with_id_and_product(topic, message_id, 10_231_453_024_539_u64)
    }

    fn make_sqs_event_with_id_and_product(
        topic: &str,
        message_id: &str,
        product_id: u64,
    ) -> LambdaEvent<SqsEvent> {
        let eb_event = make_eb_event_with_product_id(topic, product_id);
        let body = serde_json::to_string(&eb_event).unwrap();
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id.to_string());
        msg.body = Some(body);
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        LambdaEvent::new(sqs_event, Context::default())
    }

    fn partnered_shop() -> Shop {
        let mut shop: Shop = Faker.fake();
        shop.shop_id = ShopId::new();
        shop.shopify_domain = Some(Domain::try_from("partner-shop.myshopify.com").unwrap());
        shop.partner_status = ShopPartnerStatus::Partnered;
        shop
    }

    #[tokio::test]
    async fn should_upsert_product_when_shopify_event_is_valid_for_handler() {
        let shop = partnered_shop();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));
        let mut product_service = MockCommandProductService::default();
        product_service.expect_upsert().return_once(|cmds| {
            Box::pin(async move {
                assert_eq!(cmds.len(), 1);
                vec![]
            })
        });

        let result = handler(
            make_sqs_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
            &shop_service,
            &product_service,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_event_when_shop_is_not_partner_for_handler() {
        let mut shop = partnered_shop();
        shop.partner_status = ShopPartnerStatus::Scraped;
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));
        let mut product_service = MockCommandProductService::default();
        product_service.expect_upsert().never();

        let result = handler(
            make_sqs_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
            &shop_service,
            &product_service,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_partial_failure_when_product_upsert_fails_for_handler() {
        let shop = partnered_shop();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));
        let mut product_service = MockCommandProductService::default();
        product_service
            .expect_upsert()
            .return_once(|cmds| Box::pin(async move { cmds }));

        let result = handler(
            make_sqs_event_with_id(SHOPIFY_TOPIC_PRODUCTS_UPDATE, "failing-msg"),
            &shop_service,
            &product_service,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!("failing-msg", result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_report_partial_failure_while_other_messages_succeed_for_handler() {
        let shop = partnered_shop();
        let mut shop_service = MockGetShopService::default();
        // Both messages use the same shop domain — two shop lookups expected.
        shop_service
            .expect_find_shop_by_shopify_domain()
            .times(2)
            .returning(move |_| {
                let s = shop.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut product_service = MockCommandProductService::default();
        // Single batched upsert call; only the second command (product 222) fails.
        product_service.expect_upsert().once().returning(|cmds| {
            Box::pin(async move {
                // Return only the second command as a failure.
                cmds.into_iter().skip(1).take(1).collect()
            })
        });

        // Two messages with DIFFERENT product IDs so they produce distinct ProductKeys.
        let body1 = serde_json::to_string(&make_eb_event_with_product_id(
            SHOPIFY_TOPIC_PRODUCTS_CREATE,
            111,
        ))
        .unwrap();
        let body2 = serde_json::to_string(&make_eb_event_with_product_id(
            SHOPIFY_TOPIC_PRODUCTS_UPDATE,
            222,
        ))
        .unwrap();

        let mut msg1 = SqsMessage::default();
        msg1.message_id = Some("success-msg".to_string());
        msg1.body = Some(body1);

        let mut msg2 = SqsMessage::default();
        msg2.message_id = Some("fail-msg".to_string());
        msg2.body = Some(body2);

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg1, msg2];
        let event = LambdaEvent::new(sqs_event, Context::default());

        let result = handler(event, &shop_service, &product_service)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!("fail-msg", result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_fail_message_when_body_is_invalid_json_for_handler() {
        let shop_service = MockGetShopService::default();
        let product_service = MockCommandProductService::default();

        let mut msg = SqsMessage::default();
        msg.message_id = Some("bad-json-msg".to_string());
        msg.body = Some("not valid json {".to_string());

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let event = LambdaEvent::new(sqs_event, Context::default());

        let result = handler(event, &shop_service, &product_service)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            "bad-json-msg",
            result.batch_item_failures[0].item_identifier
        );
    }

    #[tokio::test]
    async fn should_skip_message_when_body_is_empty_for_handler() {
        let shop_service = MockGetShopService::default();
        let product_service = MockCommandProductService::default();

        let mut msg = SqsMessage::default();
        msg.message_id = Some("empty-msg".to_string());
        msg.body = None;

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let event = LambdaEvent::new(sqs_event, Context::default());

        let result = handler(event, &shop_service, &product_service)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
