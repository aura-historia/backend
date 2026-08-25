mod types;

pub use types::{
    ShopifyEventDetail, ShopifyEventMetadata, ShopifyImagePayload, ShopifyProductEventError,
    ShopifyProductEventKind, ShopifyProductPayload, ShopifyVariantPayload,
    fallbacked_html_to_markdown, product_availability,
};

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::LambdaEvent;
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_service::use_cases::{
    IngestShopifyProductListingError, IngestShopifyProductListingUseCase,
    WithdrawProductListingError, WithdrawProductListingUseCase,
};
use serde_json::Value;
use shop_core::domain::Domain;
use shop_core::partner_status::ShopPartnerStatus;
use shop_service::use_cases::{GetShopError, GetShopRequest, GetShopUseCase};
use tracing::{info, warn};

pub const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
pub const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
pub const SHOPIFY_TOPIC_PRODUCTS_DELETE: &str = "products/delete";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageOutcome {
    Acknowledged,
    Retry,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductListingProcessingError {
    #[error("Shopify product payload is invalid")]
    InvalidPayload(#[source] ShopifyProductEventError),
    #[error("Shopify product ingestion failed")]
    Ingestion(#[source] IngestShopifyProductListingError),
    #[error("Shop lookup failed")]
    ShopLookup(#[source] GetShopError),
    #[error("Shopify product listing withdrawal failed")]
    Withdrawal(#[source] WithdrawProductListingError),
}

#[async_trait::async_trait]
pub trait ShopifyProductListingProcessorUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        shop_domain: Domain,
        payload: ShopifyProductPayload,
    ) -> Result<(), ShopifyProductListingProcessingError>;
}

pub struct ShopifyProductListingProcessor<S, I, W> {
    shops: S,
    ingestion: I,
    withdrawal: W,
}

impl<S, I, W> ShopifyProductListingProcessor<S, I, W> {
    pub fn new(shops: S, ingestion: I, withdrawal: W) -> Self {
        Self {
            shops,
            ingestion,
            withdrawal,
        }
    }
}

#[async_trait::async_trait]
impl<S, I, W> ShopifyProductListingProcessorUseCase for ShopifyProductListingProcessor<S, I, W>
where
    S: GetShopUseCase,
    I: IngestShopifyProductListingUseCase,
    W: WithdrawProductListingUseCase,
{
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        shop_domain: Domain,
        payload: ShopifyProductPayload,
    ) -> Result<(), ShopifyProductListingProcessingError> {
        match kind {
            ShopifyProductEventKind::Create | ShopifyProductEventKind::Update => self
                .ingestion
                .execute(
                    context,
                    kind.command(shop_domain, payload)
                        .map_err(ShopifyProductListingProcessingError::InvalidPayload)?,
                )
                .await
                .map(|_| ())
                .map_err(ShopifyProductListingProcessingError::Ingestion),
            ShopifyProductEventKind::Delete => {
                let shop = match self
                    .shops
                    .execute(context, GetShopRequest::ByShopifyDomain(shop_domain))
                    .await
                {
                    Ok(shop) if shop.partner_status == ShopPartnerStatus::Partnered => shop,
                    Ok(_) | Err(GetShopError::NotFound) => return Ok(()),
                    Err(error) => {
                        return Err(ShopifyProductListingProcessingError::ShopLookup(error));
                    }
                };
                let key = ProductListingKey::new(
                    shop.shop_id,
                    ShopListingId::from(payload.id.to_string()),
                );
                match self.withdrawal.execute_by_key(context, key).await {
                    Ok(_) | Err(WithdrawProductListingError::NotFound) => Ok(()),
                    Err(error) => Err(ShopifyProductListingProcessingError::Withdrawal(error)),
                }
            }
        }
    }
}

#[tracing::instrument(
    skip(event, processor),
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
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
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
    match processor
        .execute(context, kind, shop_domain, detail.payload)
        .await
    {
        Ok(()) => MessageOutcome::Acknowledged,
        Err(error) if should_retry(&error) => {
            warn!(%error, "Shopify product processing failed; retrying SQS message");
            MessageOutcome::Retry
        }
        Err(error) => {
            warn!(%error, "Shopify product payload cannot be processed; acknowledging message");
            MessageOutcome::Acknowledged
        }
    }
}

fn should_retry(error: &ShopifyProductListingProcessingError) -> bool {
    match error {
        ShopifyProductListingProcessingError::InvalidPayload(_) => false,
        ShopifyProductListingProcessingError::Ingestion(error) => !matches!(
            error,
            IngestShopifyProductListingError::MissingTitle
                | IngestShopifyProductListingError::MissingHandle
                | IngestShopifyProductListingError::InvalidPrice
                | IngestShopifyProductListingError::InvalidProductListingUrl
        ),
        ShopifyProductListingProcessingError::ShopLookup(_)
        | ShopifyProductListingProcessingError::Withdrawal(_) => true,
    }
}

#[tracing::instrument(skip(event, processor), fields(request_id = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
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
        if process_event(event, &context, processor).await == MessageOutcome::Retry {
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
    use product_listing_service::use_cases::IngestShopifyProductListingError;
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
        assert!(!should_retry(
            &ShopifyProductListingProcessingError::Ingestion(
                IngestShopifyProductListingError::InvalidPrice
            )
        ));
        assert!(!should_retry(
            &ShopifyProductListingProcessingError::Ingestion(
                IngestShopifyProductListingError::MissingTitle
            )
        ));
        assert!(should_retry(
            &ShopifyProductListingProcessingError::Ingestion(
                IngestShopifyProductListingError::MissingShopCurrency
            )
        ));
        assert!(should_retry(
            &ShopifyProductListingProcessingError::Ingestion(
                IngestShopifyProductListingError::ShopLookupInternal
            )
        ));
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
    impl ShopifyProductListingProcessorUseCase for FakeIngestion {
        async fn execute(
            &self,
            _context: &OperationContext,
            _kind: ShopifyProductEventKind,
            _shop_domain: Domain,
            _payload: ShopifyProductPayload,
        ) -> Result<(), ShopifyProductListingProcessingError> {
            *self.calls.lock().unwrap_or_else(|error| error.into_inner()) += 1;
            match self.result {
                FakeResult::Success => Ok(()),
                FakeResult::Failure => Err(ShopifyProductListingProcessingError::Ingestion(
                    IngestShopifyProductListingError::ShopLookupInternal,
                )),
            }
        }
    }
}
