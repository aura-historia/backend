mod types;

pub use types::{
    ShopifyEventDetail, ShopifyEventMetadata, ShopifyImagePayload, ShopifyProductEventError,
    ShopifyProductEventKind, ShopifyProductPayload, ShopifyVariantPayload,
    fallbacked_html_to_markdown, product_state,
};

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::LambdaEvent;
use product_listing_service::use_cases::{
    IngestShopifyProductError, IngestShopifyProductResult, IngestShopifyProductUseCase,
};
use serde_json::Value;
use shop_core::domain::Domain;
use tracing::{info, warn};

pub const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
pub const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
pub const SHOPIFY_TOPIC_PRODUCTS_DELETE: &str = "products/delete";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageOutcome {
    Acknowledged,
    Retry,
}

#[tracing::instrument(
    skip(event, ingestion),
    fields(
        event_bridge_event_id = tracing::field::Empty,
        shopify_event_id = tracing::field::Empty,
        shopify_topic = tracing::field::Empty,
        shopify_domain = tracing::field::Empty,
    )
)]
async fn process_event(
    event: EventBridgeEvent<Value>,
    context: &OperationContext,
    ingestion: &(dyn IngestShopifyProductUseCase + Send + Sync),
) -> MessageOutcome {
    let span = tracing::Span::current();
    if let Some(event_id) = event.id.as_deref() {
        span.record("event_bridge_event_id", event_id);
    }
    let detail = match serde_json::from_value::<ShopifyEventDetail>(event.detail) {
        Ok(detail) => detail,
        Err(error) => {
            warn!(%error, "Shopify event detail is malformed; retrying SQS message");
            return MessageOutcome::Retry;
        }
    };
    if let Some(event_id) = detail.metadata.event_id.as_deref() {
        span.record("shopify_event_id", event_id);
    }
    span.record("shopify_topic", detail.metadata.topic.as_str());
    span.record("shopify_domain", detail.metadata.shop_domain.as_str());

    let kind = match detail.metadata.topic.as_str() {
        SHOPIFY_TOPIC_PRODUCTS_CREATE => ShopifyProductEventKind::Create,
        SHOPIFY_TOPIC_PRODUCTS_UPDATE => ShopifyProductEventKind::Update,
        SHOPIFY_TOPIC_PRODUCTS_DELETE => ShopifyProductEventKind::Delete,
        _ => return MessageOutcome::Acknowledged,
    };
    let shop_domain = match Domain::try_from(detail.metadata.shop_domain.as_str()) {
        Ok(domain) => domain,
        Err(error) => {
            warn!(%error, "Shopify event has invalid shop domain; acknowledging message");
            return MessageOutcome::Acknowledged;
        }
    };
    let command = match kind.command(shop_domain, detail.payload) {
        Ok(command) => command,
        Err(error) => {
            warn!(%error, "Shopify product payload cannot be ingested; acknowledging message");
            return MessageOutcome::Acknowledged;
        }
    };

    match ingestion.execute(context, command).await {
        Ok(IngestShopifyProductResult::Ignored) => MessageOutcome::Acknowledged,
        Ok(IngestShopifyProductResult::Upserted(_)) => MessageOutcome::Acknowledged,
        Err(error) if should_retry(&error) => {
            warn!(%error, "Shopify product ingestion failed; retrying SQS message");
            MessageOutcome::Retry
        }
        Err(error) => {
            warn!(%error, "Shopify product payload cannot be ingested; acknowledging message");
            MessageOutcome::Acknowledged
        }
    }
}

fn should_retry(error: &IngestShopifyProductError) -> bool {
    !matches!(
        error,
        IngestShopifyProductError::MissingTitle
            | IngestShopifyProductError::MissingHandle
            | IngestShopifyProductError::InvalidPrice
            | IngestShopifyProductError::InvalidProductUrl
    )
}

#[tracing::instrument(skip(event, ingestion), fields(request_id = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    ingestion: &(dyn IngestShopifyProductUseCase + Send + Sync),
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let context = operation_context(&event);
    let count = event.payload.records.len();
    let mut failed_message_ids = Vec::new();

    for message in event.payload.records {
        let Some(message_id) = message.message_id else {
            warn!("Shopify SQS message has no message ID; acknowledging message");
            continue;
        };
        let Some(body) = message.body else {
            continue;
        };
        let event = match serde_json::from_str::<EventBridgeEvent<Value>>(&body) {
            Ok(event) => event,
            Err(error) => {
                warn!(message_id = %message_id, %error, "Shopify SQS body is malformed; retrying message");
                failed_message_ids.push(message_id);
                continue;
            }
        };
        if process_event(event, &context, ingestion).await == MessageOutcome::Retry {
            failed_message_ids.push(message_id);
        }
    }

    info!(
        sqs_message_count = count,
        failed_sqs_message_count = failed_message_ids.len(),
        "Finished Shopify product ingestion batch"
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

fn operation_context(event: &LambdaEvent<SqsEvent>) -> OperationContext {
    let request_id = RequestId::new(event.context.request_id.clone());
    OperationContext {
        principal: Principal::System,
        correlation_id: CorrelationId::new(request_id.as_str()),
        request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use product_listing_service::use_cases::{
        IngestShopifyProductCommand, IngestShopifyProductError, IngestShopifyProductResult,
    };
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn should_acknowledge_valid_shopify_message() {
        let ingestion = FakeIngestion::success();
        let result = handler(event("msg-1", valid_body()), &ingestion)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
        assert_eq!(
            1,
            *ingestion
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
        );
    }

    #[tokio::test]
    async fn should_retry_when_ingestion_fails() {
        let ingestion = FakeIngestion::failure();
        let result = handler(event("msg-1", valid_body()), &ingestion)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["msg-1"], identifiers(result));
    }

    #[tokio::test]
    async fn should_retry_when_sqs_body_is_invalid() {
        let result = handler(
            event("msg-1", "not JSON".to_owned()),
            &FakeIngestion::success(),
        )
        .await
        .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["msg-1"], identifiers(result));
    }

    #[test]
    fn should_not_retry_permanently_invalid_shopify_payload() {
        assert!(!should_retry(&IngestShopifyProductError::InvalidPrice));
        assert!(!should_retry(&IngestShopifyProductError::MissingTitle));
        assert!(should_retry(
            &IngestShopifyProductError::MissingShopCurrency
        ));
        assert!(should_retry(&IngestShopifyProductError::ShopLookupInternal));
    }

    #[tokio::test]
    async fn should_acknowledge_unsupported_topic_without_ingestion() {
        let ingestion = FakeIngestion::success();
        let result = handler(event("msg-1", body_with_topic("orders/create")), &ingestion)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
        assert_eq!(
            0,
            *ingestion
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
        );
    }

    fn event(message_id: &str, body: String) -> LambdaEvent<SqsEvent> {
        let mut message = SqsMessage::default();
        message.message_id = Some(message_id.to_owned());
        message.body = Some(body);
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![message];
        LambdaEvent::new(sqs_event, Context::default())
    }

    fn valid_body() -> String {
        body_with_topic(SHOPIFY_TOPIC_PRODUCTS_CREATE)
    }

    fn body_with_topic(topic: &str) -> String {
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = "shopifyWebhook".to_owned();
        event.source = "aws.partner/shopify.com/test".to_owned();
        event.detail = serde_json::json!({
            "payload": {
                "id": 42,
                "title": "Cabinet",
                "handle": "cabinet",
                "status": "active",
                "variants": [{"price": "42.00", "inventory_quantity": 1}],
                "images": []
            },
            "metadata": {
                "X-Shopify-Topic": topic,
                "X-Shopify-Shop-Domain": "partner.example"
            }
        });
        serde_json::to_string(&event)
            .unwrap_or_else(|error| panic!("failed serializing EventBridge fixture: {error}"))
    }

    fn identifiers(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Success,
        Failure,
    }

    #[derive(Clone)]
    struct FakeIngestion {
        calls: Arc<Mutex<usize>>,
        result: FakeResult,
    }

    impl FakeIngestion {
        fn success() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::Success,
            }
        }

        fn failure() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::Failure,
            }
        }
    }

    #[async_trait::async_trait]
    impl IngestShopifyProductUseCase for FakeIngestion {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: IngestShopifyProductCommand,
        ) -> Result<IngestShopifyProductResult, IngestShopifyProductError> {
            *self.calls.lock().unwrap_or_else(|error| error.into_inner()) += 1;
            match self.result {
                FakeResult::Success => Ok(IngestShopifyProductResult::Ignored),
                FakeResult::Failure => Err(IngestShopifyProductError::ShopLookupInternal),
            }
        }
    }
}
