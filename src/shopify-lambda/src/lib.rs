mod types;

pub use types::{
    ShopifyEventDetail, ShopifyEventMetadata, ShopifyImagePayload, ShopifyListingAction,
    ShopifyProductEventError, ShopifyProductEventKind, ShopifyProductPayload,
    ShopifyRawObservation, ShopifyVariantPayload, product_availability,
    source_occurred_at_from_triggered_at,
};

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::LambdaEvent;
use listing_source_core::Domain;
use listing_source_service::ports::{ListingSourceReadError, ShopifySourceReader};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use product_listing_ingestion_sqs::with_publication_deadline;
use product_listing_normalization::{RawProductListingProvenance, SourcePayload};
use product_listing_service::ports::{
    ProductListingRawIngestionMethod, ProductListingRawProviderReceipt,
    ProviderReceiptDeliveryIdError, ProviderReceiptScope, ProviderReceiptScopeError,
    SourceEvidenceSha256,
};
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationCommand, IndexedProductListingIngestionIntent,
    ProductListingIngestionActor, ProductListingIngestionIdempotencyKey,
    ProductListingIngestionIntent, ProductListingIngestionOperation,
    ProductListingIngestionOutcome, ProductListingIngestionSubmission,
    ProductListingIngestionSubmissionError, SubmitInternalProductListingIngestionUseCase,
    product_listing_ingestion_identity,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tracing::{info, warn};

pub const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
pub const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
pub const SHOPIFY_TOPIC_PRODUCTS_DELETE: &str = "products/delete";

const SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX: &str = "shopify-webhook:";
const EVENTBRIDGE_RECEIPT_DELIVERY_ID_PREFIX: &str = "eventbridge:";
const SQS_DELIVERY_ID_PREFIX: &str = "sqs:";
const SHOPIFY_SUBMISSION_IDENTITY_DOMAIN: &[u8] = b"aura.shopify.ingestion.delivery.v1";
const INVOCATION_BUDGET_CAP: Duration = Duration::from_secs(30);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(1);
const FORWARD_REPORT_HEADROOM: Duration = Duration::from_millis(300);

/// Capture this before credential refresh; per-record scopes can only shorten it.
pub fn publication_deadline(context: &lambda_runtime::Context) -> tokio::time::Instant {
    let budget =
        LambdaInvocationBudget::from_context(context, INVOCATION_BUDGET_CAP, RESPONSE_HEADROOM);
    tokio::time::Instant::now() + budget.remaining().saturating_sub(FORWARD_REPORT_HEADROOM)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageOutcome {
    Acknowledged,
    Retry,
}

#[derive(Debug, Clone)]
pub struct ShopifyEventProvenance {
    pub topic: String,
    pub shopify_event_id: Option<String>,
    pub webhook_id: Option<String>,
    pub event_bridge_event_id: Option<String>,
    pub triggered_at: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProviderReceiptError {
    #[error("Shopify provider receipt scope is invalid")]
    Scope(#[source] ProviderReceiptScopeError),
    #[error("Shopify provider receipt delivery ID is invalid")]
    DeliveryId(#[source] ProviderReceiptDeliveryIdError),
    #[error("Shopify provider receipt source payload is invalid")]
    SourcePayload(#[source] product_listing_normalization::NormalizationInputError),
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductListingProcessingError {
    #[error("Shopify product payload is invalid")]
    InvalidPayload(#[source] ShopifyProductEventError),
    #[error("Shopify raw product provenance is invalid")]
    InvalidProvenance(#[source] product_listing_normalization::NormalizationInputError),
    #[error("Shopify provider receipt is invalid")]
    InvalidProviderReceipt(#[source] ShopifyProviderReceiptError),
    #[error("Listing source lookup failed")]
    ListingSourceLookup(#[source] ListingSourceReadError),
    #[error("Shopify product listing submission failed")]
    Submit(#[source] ProductListingIngestionSubmissionError),
    #[error("Shopify product listing forward was not confirmed")]
    ForwardUnconfirmed,
    #[error("Shopify submission identity is invalid")]
    InvalidSubmissionIdentity,
}

#[async_trait::async_trait]
pub trait ShopifyProductListingProcessorUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        shop_domain: Domain,
        payload: Value,
        provenance: ShopifyEventProvenance,
        upstream_message_id: &str,
    ) -> Result<(), ShopifyProductListingProcessingError>;
}

pub struct ShopifyProductListingProcessor<S, I> {
    sources: S,
    intake: I,
}

impl<S, I> ShopifyProductListingProcessor<S, I> {
    pub fn new(sources: S, intake: I) -> Self {
        Self { sources, intake }
    }
}

#[async_trait::async_trait]
impl<S, I> ShopifyProductListingProcessorUseCase for ShopifyProductListingProcessor<S, I>
where
    S: ShopifySourceReader,
    I: SubmitInternalProductListingIngestionUseCase,
{
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        source_domain: Domain,
        payload: Value,
        provenance: ShopifyEventProvenance,
        upstream_message_id: &str,
    ) -> Result<(), ShopifyProductListingProcessingError> {
        let Some(source) = self
            .sources
            .find_by_domain(&source_domain)
            .await
            .map_err(ShopifyProductListingProcessingError::ListingSourceLookup)?
        else {
            return Ok(());
        };
        let ShopifyListingAction::Capture(mut observation) = kind
            .listing_action(&source, payload)
            .map_err(ShopifyProductListingProcessingError::InvalidPayload)?
        else {
            return Ok(());
        };
        observation.source_occurred_at =
            source_occurred_at_from_triggered_at(provenance.triggered_at.as_deref())
                .map_err(ShopifyProductListingProcessingError::InvalidPayload)?;
        let provider_receipt = shopify_provider_receipt(
            provenance.topic.as_str(),
            provenance.webhook_id.as_deref(),
            provenance.event_bridge_event_id.as_deref(),
            observation.input.source_payload(),
        )
        .map_err(ShopifyProductListingProcessingError::InvalidProviderReceipt)?;
        let raw_provenance = RawProductListingProvenance::new(json!({
            "topic": &provenance.topic,
            "shopifyEventId": &provenance.shopify_event_id,
            "shopifyWebhookId": &provenance.webhook_id,
            "eventBridgeEventId": &provenance.event_bridge_event_id,
            "shopifyTriggeredAt": &provenance.triggered_at,
        }))
        .map_err(ShopifyProductListingProcessingError::InvalidProvenance)?;

        let idempotency_key = shopify_submission_key(&provenance, upstream_message_id)?;
        let actor = match &context.principal {
            Principal::Service(service_id) => {
                ProductListingIngestionActor::Service(service_id.clone())
            }
            _ => ProductListingIngestionActor::System,
        };
        let (expected_submission_id, expected_command_id) = product_listing_ingestion_identity(
            &actor,
            source.listing_source_id,
            &idempotency_key,
            ProductListingIngestionOperation::CaptureRaw,
            0,
        );
        let submission = ProductListingIngestionSubmission {
            listing_source_id: source.listing_source_id,
            original_input_count: 1,
            idempotency_key: Some(idempotency_key.clone()),
            items: vec![IndexedProductListingIngestionIntent {
                index: 0,
                intent: ProductListingIngestionIntent::CaptureRaw(
                    CaptureProductListingRawObservationCommand {
                        listing_source_id: source.listing_source_id,
                        ingestion_method: ProductListingRawIngestionMethod::Shopify,
                        source_record_key: observation.source_record_key,
                        input: observation.input,
                        provenance: raw_provenance,
                        source_event_id: provenance.shopify_event_id,
                        source_occurred_at: observation.source_occurred_at,
                        provider_receipt,
                    },
                ),
            }],
        };
        let result = self
            .intake
            .execute(context, submission)
            .await
            .map_err(ShopifyProductListingProcessingError::Submit)?;
        if result.original_input_count == 1
            && result.idempotency_key == idempotency_key
            && result.submission_id == expected_submission_id
            && result.items.len() == 1
            && result.items[0].index == 0
            && result.items[0].command_id == expected_command_id
            && matches!(
                result.items[0].outcome,
                ProductListingIngestionOutcome::Accepted
            )
        {
            info!(
                forwarding_outcome = "accepted",
                "Shopify ingestion forward confirmed"
            );
            Ok(())
        } else {
            let reason = match result.items.first().map(|item| &item.outcome) {
                Some(ProductListingIngestionOutcome::Rejected { .. }) => "rejected",
                Some(ProductListingIngestionOutcome::Unconfirmed) => "unconfirmed",
                Some(ProductListingIngestionOutcome::NotAttempted { .. }) => "not_attempted",
                _ => "invalid_report",
            };
            warn!(
                forwarding_outcome = reason,
                "Shopify ingestion forward not confirmed"
            );
            Err(ShopifyProductListingProcessingError::ForwardUnconfirmed)
        }
    }
}

#[tracing::instrument(
    skip(event, processor),
    fields(
        event_bridge_event_id = tracing::field::Empty,
        shopify_event_id = tracing::field::Empty,
        shopify_webhook_id = tracing::field::Empty,
        shopify_topic = tracing::field::Empty,
        shopify_domain = tracing::field::Empty,
    )
)]
async fn process_event(
    event: EventBridgeEvent<Value>,
    context: &OperationContext,
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
    upstream_message_id: &str,
) -> MessageOutcome {
    let span = tracing::Span::current();
    if let Some(event_id) = event.id.as_deref() {
        span.record("event_bridge_event_id", event_id);
    }
    let event_bridge_event_id = event.id;
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
    if let Some(webhook_id) = detail.metadata.webhook_id.as_deref() {
        span.record("shopify_webhook_id", webhook_id);
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
    let provenance = ShopifyEventProvenance {
        topic: detail.metadata.topic,
        shopify_event_id: detail.metadata.event_id,
        webhook_id: detail.metadata.webhook_id,
        event_bridge_event_id,
        triggered_at: detail.metadata.triggered_at,
    };
    match processor
        .execute(
            context,
            kind,
            shop_domain,
            detail.payload,
            provenance,
            upstream_message_id,
        )
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
        ShopifyProductListingProcessingError::InvalidPayload(_)
        | ShopifyProductListingProcessingError::InvalidProvenance(_)
        | ShopifyProductListingProcessingError::InvalidProviderReceipt(_) => false,
        ShopifyProductListingProcessingError::ListingSourceLookup(_)
        | ShopifyProductListingProcessingError::Submit(_)
        | ShopifyProductListingProcessingError::ForwardUnconfirmed
        | ShopifyProductListingProcessingError::InvalidSubmissionIdentity => true,
    }
}

fn shopify_provider_receipt(
    topic: &str,
    webhook_id: Option<&str>,
    event_bridge_event_id: Option<&str>,
    source_payload: &SourcePayload,
) -> Result<Option<ProductListingRawProviderReceipt>, ShopifyProviderReceiptError> {
    let Some(delivery_id) = shopify_receipt_delivery_identity(webhook_id, event_bridge_event_id)
        .map_err(ShopifyProviderReceiptError::DeliveryId)?
    else {
        return Ok(None);
    };
    let scope =
        ProviderReceiptScope::new(topic.to_owned()).map_err(ShopifyProviderReceiptError::Scope)?;
    let source_evidence_sha256 = source_payload
        .canonical_sha256()
        .map_err(ShopifyProviderReceiptError::SourcePayload)?;
    ProductListingRawProviderReceipt::new(
        scope,
        delivery_id,
        SourceEvidenceSha256::new(*source_evidence_sha256.as_bytes()),
    )
    .map(Some)
    .map_err(ShopifyProviderReceiptError::DeliveryId)
}

fn shopify_receipt_delivery_identity(
    webhook_id: Option<&str>,
    event_bridge_event_id: Option<&str>,
) -> Result<Option<String>, ProviderReceiptDeliveryIdError> {
    let (prefix, delivery_id) = match webhook_id {
        Some(webhook_id) => (SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX, webhook_id),
        None => match event_bridge_event_id {
            Some(event_bridge_event_id) => (
                EVENTBRIDGE_RECEIPT_DELIVERY_ID_PREFIX,
                event_bridge_event_id,
            ),
            None => return Ok(None),
        },
    };
    if delivery_id.is_empty() {
        return Err(ProviderReceiptDeliveryIdError::Empty);
    }

    Ok(Some(format!("{prefix}{delivery_id}")))
}

fn shopify_submission_key(
    provenance: &ShopifyEventProvenance,
    upstream_message_id: &str,
) -> Result<ProductListingIngestionIdempotencyKey, ShopifyProductListingProcessingError> {
    let delivery = shopify_receipt_delivery_identity(
        provenance.webhook_id.as_deref(),
        provenance.event_bridge_event_id.as_deref(),
    )
    .map_err(|error| {
        ShopifyProductListingProcessingError::InvalidProviderReceipt(
            ShopifyProviderReceiptError::DeliveryId(error),
        )
    })?
    .unwrap_or_else(|| format!("{SQS_DELIVERY_ID_PREFIX}{upstream_message_id}"));
    let mut hasher = Sha256::new();
    for field in [
        SHOPIFY_SUBMISSION_IDENTITY_DOMAIN,
        provenance.topic.as_bytes(),
        delivery.as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    let digest = hasher.finalize();
    let mut key = String::from("shopify-");
    for byte in digest {
        key.push_str(&format!("{byte:02x}"));
    }
    ProductListingIngestionIdempotencyKey::new(key)
        .map_err(|_| ShopifyProductListingProcessingError::InvalidSubmissionIdentity)
}

#[tracing::instrument(skip(event, processor), fields(request_id = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let context = operation_context(&event);
    let budget = LambdaInvocationBudget::from_context(
        &event.context,
        INVOCATION_BUDGET_CAP,
        RESPONSE_HEADROOM,
    );
    let count = event.payload.records.len();
    let mut failed_message_ids = Vec::new();

    for message in event.payload.records {
        let Some(message_id) = message.message_id else {
            warn!("Shopify SQS message has no message ID; acknowledging message");
            continue;
        };
        if budget.remaining().is_zero() {
            failed_message_ids.push(message_id);
            continue;
        }
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
        match tokio::time::timeout(
            budget.remaining(),
            with_publication_deadline(
                tokio::time::Instant::now()
                    + budget.remaining().saturating_sub(FORWARD_REPORT_HEADROOM),
                process_event(event, &context, processor, &message_id),
            ),
        )
        .await
        {
            Ok(MessageOutcome::Acknowledged) => {}
            Ok(MessageOutcome::Retry) => failed_message_ids.push(message_id),
            Err(_) => {
                warn!(
                    "Shopify ingestion forward exceeded invocation budget; retrying upstream SQS message"
                );
                failed_message_ids.push(message_id);
            }
        }
    }

    info!(
        sqs_message_count = count,
        failed_sqs_message_count = failed_message_ids.len(),
        "Finished Shopify ingestion forwarding batch"
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
    use std::{
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[tokio::test]
    async fn should_acknowledge_valid_shopify_message() {
        let processor = FakeProcessor::success();
        let result = handler(event("msg-1", valid_body()), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
        assert_eq!(1, call_count(&processor));
    }

    #[tokio::test]
    async fn should_retry_when_forward_is_unconfirmed() {
        let processor = FakeProcessor::failure();
        let result = handler(event("msg-1", valid_body()), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["msg-1"], identifiers(result));
    }

    #[test]
    fn should_retry_every_unconfirmed_submission() {
        assert!(should_retry(
            &ShopifyProductListingProcessingError::ForwardUnconfirmed
        ));
        assert!(should_retry(&ShopifyProductListingProcessingError::Submit(
            ProductListingIngestionSubmissionError::InconsistentInputIndices,
        )));
    }

    #[tokio::test]
    async fn should_only_fail_unconfirmed_record_in_shopify_batch() {
        let processor = FakeProcessor::unconfirmed_on_second_call();
        let result = handler(
            events(vec![("valid", valid_body()), ("conflicted", valid_body())]),
            &processor,
        )
        .await
        .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["conflicted"], identifiers(result));
        assert_eq!(2, call_count(&processor));
    }

    #[test]
    fn should_namespace_receipt_delivery_identity_by_origin() {
        let webhook_identity =
            shopify_receipt_delivery_identity(Some("same-delivery-id"), Some("same-delivery-id"))
                .unwrap_or_else(|error| panic!("webhook identity failed: {error}"));
        let eventbridge_identity =
            shopify_receipt_delivery_identity(None, Some("same-delivery-id"))
                .unwrap_or_else(|error| panic!("EventBridge identity failed: {error}"));

        assert_eq!(
            Some("shopify-webhook:same-delivery-id".to_owned()),
            webhook_identity
        );
        assert_eq!(
            Some("eventbridge:same-delivery-id".to_owned()),
            eventbridge_identity
        );
        assert!(matches!(
            shopify_receipt_delivery_identity(Some(""), None),
            Err(ProviderReceiptDeliveryIdError::Empty)
        ));
    }

    #[tokio::test]
    async fn should_retry_when_sqs_body_is_invalid() {
        let result = handler(
            event("msg-1", "not JSON".to_owned()),
            &FakeProcessor::success(),
        )
        .await
        .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["msg-1"], identifiers(result));
    }

    #[tokio::test]
    async fn should_acknowledge_malformed_product_payload() {
        let processor = FakeProcessor::invalid_payload();
        let result = handler(event("msg-1", valid_body()), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_acknowledge_unsupported_topic_without_capture() {
        let processor = FakeProcessor::success();
        let result = handler(event("msg-1", body_with_topic("orders/create")), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
        assert_eq!(0, call_count(&processor));
    }

    #[derive(Clone, Copy)]
    enum SubmissionReport {
        Accepted,
        Rejected,
        Unconfirmed,
        NotAttempted,
        Missing,
        WrongIndex,
        WrongCommand,
        WrongSubmission,
        WrongKey,
        WrongCount,
        Extra,
    }

    struct TestSource;

    #[async_trait::async_trait]
    impl ShopifySourceReader for TestSource {
        async fn find_by_domain(
            &self,
            domain: &Domain,
        ) -> Result<Option<listing_source_service::ports::ShopifySource>, ListingSourceReadError>
        {
            Ok(Some(listing_source_service::ports::ShopifySource {
                listing_source_id: listing_source_core::ListingSourceId::new(),
                domain: domain.clone(),
                currency: Some(money::Currency::Usd),
                language: None,
            }))
        }
    }

    struct TestIntake(SubmissionReport);

    #[async_trait::async_trait]
    impl SubmitInternalProductListingIngestionUseCase for TestIntake {
        async fn execute(
            &self,
            context: &OperationContext,
            submission: ProductListingIngestionSubmission,
        ) -> Result<
            product_listing_service::use_cases::ProductListingIngestionSubmissionResult,
            ProductListingIngestionSubmissionError,
        > {
            let actor = match &context.principal {
                Principal::System => ProductListingIngestionActor::System,
                Principal::Service(id) => ProductListingIngestionActor::Service(id.clone()),
                _ => panic!("trusted internal principal required"),
            };
            let key = submission.idempotency_key.expect("delivery key");
            let (mut submission_id, mut command_id) = product_listing_ingestion_identity(
                &actor,
                submission.listing_source_id,
                &key,
                ProductListingIngestionOperation::CaptureRaw,
                0,
            );
            assert!(matches!(
                submission.items[0].intent,
                ProductListingIngestionIntent::CaptureRaw(_)
            ));
            let outcome = match self.0 {
                SubmissionReport::Rejected => ProductListingIngestionOutcome::Rejected {
                    reason: product_listing_service::use_cases::ProductListingIngestionRejectionReason::Publisher {
                        code: "INVALID_MESSAGE_SIZE".to_owned(),
                    },
                    retryable: false,
                },
                SubmissionReport::Unconfirmed => ProductListingIngestionOutcome::Unconfirmed,
                SubmissionReport::NotAttempted => ProductListingIngestionOutcome::NotAttempted {
                    reason: product_listing_service::use_cases::ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                    retryable: true,
                },
                _ => ProductListingIngestionOutcome::Accepted,
            };
            if matches!(self.0, SubmissionReport::WrongCommand) {
                command_id = product_listing_service::use_cases::ProductListingIngestionCommandId::from_wire(&format!("plic1_{}", "a".repeat(64))).unwrap();
            }
            if matches!(self.0, SubmissionReport::WrongSubmission) {
                submission_id = product_listing_service::use_cases::ProductListingIngestionSubmissionId::from_wire(&format!("plis1_{}", "a".repeat(64))).unwrap();
            }
            let mut items = vec![
                product_listing_service::use_cases::ProductListingIngestionItemOutcome {
                    index: usize::from(matches!(self.0, SubmissionReport::WrongIndex)),
                    command_id,
                    outcome,
                },
            ];
            if matches!(self.0, SubmissionReport::Missing) {
                items.clear();
            }
            if matches!(self.0, SubmissionReport::Extra) {
                items.push(items[0].clone());
            }
            let idempotency_key = if matches!(self.0, SubmissionReport::WrongKey) {
                ProductListingIngestionIdempotencyKey::new("other").unwrap()
            } else {
                key
            };
            Ok(
                product_listing_service::use_cases::ProductListingIngestionSubmissionResult {
                    submission_id,
                    idempotency_key,
                    original_input_count: if matches!(self.0, SubmissionReport::WrongCount) {
                        2
                    } else {
                        1
                    },
                    items,
                },
            )
        }
    }

    #[tokio::test]
    async fn should_acknowledge_only_a_matching_accepted_single_item_report() {
        for (report, accepted) in [
            (SubmissionReport::Accepted, true),
            (SubmissionReport::Rejected, false),
            (SubmissionReport::Unconfirmed, false),
            (SubmissionReport::NotAttempted, false),
            (SubmissionReport::Missing, false),
            (SubmissionReport::WrongIndex, false),
            (SubmissionReport::WrongCommand, false),
            (SubmissionReport::WrongSubmission, false),
            (SubmissionReport::WrongKey, false),
            (SubmissionReport::WrongCount, false),
            (SubmissionReport::Extra, false),
        ] {
            let processor = ShopifyProductListingProcessor::new(TestSource, TestIntake(report));
            let response = handler(event("upstream-id", valid_body()), &processor)
                .await
                .unwrap();
            assert_eq!(identifiers(response).is_empty(), accepted);
        }
    }

    #[tokio::test]
    async fn should_accept_service_context_without_partner_authorization() {
        let processor =
            ShopifyProductListingProcessor::new(TestSource, TestIntake(SubmissionReport::Accepted));
        let context = OperationContext {
            principal: Principal::Service("shopify-lambda".to_owned()),
            correlation_id: CorrelationId::new("correlation"),
            request_id: RequestId::new("request"),
        };
        let event: EventBridgeEvent<Value> = serde_json::from_str(&valid_body()).unwrap();
        let detail: ShopifyEventDetail = serde_json::from_value(event.detail).unwrap();
        let result = processor
            .execute(
                &context,
                ShopifyProductEventKind::Create,
                Domain::try_from("partner.example").unwrap(),
                detail.payload,
                ShopifyEventProvenance {
                    topic: SHOPIFY_TOPIC_PRODUCTS_CREATE.to_owned(),
                    shopify_event_id: None,
                    webhook_id: None,
                    event_bridge_event_id: event.id,
                    triggered_at: None,
                },
                "upstream-id",
            )
            .await;
        assert!(
            result.is_ok(),
            "internal Service intake should forward: {result:?}"
        );
    }

    #[test]
    fn delivery_identity_is_scoped_to_topic_and_not_batch_position() {
        let provenance = |topic: &str, webhook: Option<&str>, event_bridge: Option<&str>| {
            ShopifyEventProvenance {
                topic: topic.to_owned(),
                shopify_event_id: Some("same-content-event".to_owned()),
                webhook_id: webhook.map(str::to_owned),
                event_bridge_event_id: event_bridge.map(str::to_owned),
                triggered_at: None,
            }
        };
        let key = |p: ShopifyEventProvenance, sqs: &str| shopify_submission_key(&p, sqs).unwrap();
        assert_eq!(
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, Some("webhook"), None),
                "first"
            ),
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, Some("webhook"), None),
                "second"
            )
        );
        assert_ne!(
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, Some("webhook"), None),
                "first"
            ),
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_UPDATE, Some("webhook"), None),
                "first"
            )
        );
        assert_ne!(
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, Some("webhook"), None),
                "first"
            ),
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, None, Some("webhook")),
                "first"
            )
        );
        assert_ne!(
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, None, None),
                "first"
            ),
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, None, None),
                "second"
            )
        );
        assert_ne!(
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, None, None),
                "first"
            ),
            key(
                provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, None, Some("first")),
                "first"
            )
        );
        let delivery_key = key(
            provenance(SHOPIFY_TOPIC_PRODUCTS_CREATE, Some("webhook"), None),
            "first",
        );
        let first_source = listing_source_core::ListingSourceId::new();
        let second_source = listing_source_core::ListingSourceId::new();
        let (_, first_command) = product_listing_ingestion_identity(
            &ProductListingIngestionActor::System,
            first_source,
            &delivery_key,
            ProductListingIngestionOperation::CaptureRaw,
            0,
        );
        let (_, second_command) = product_listing_ingestion_identity(
            &ProductListingIngestionActor::System,
            second_source,
            &delivery_key,
            ProductListingIngestionOperation::CaptureRaw,
            0,
        );
        assert_ne!(first_command, second_command);
    }

    struct MixedBatchProcessor(Arc<Mutex<usize>>);

    #[async_trait::async_trait]
    impl ShopifyProductListingProcessorUseCase for MixedBatchProcessor {
        async fn execute(
            &self,
            _context: &OperationContext,
            _kind: ShopifyProductEventKind,
            _shop_domain: Domain,
            _payload: Value,
            _provenance: ShopifyEventProvenance,
            _upstream_message_id: &str,
        ) -> Result<(), ShopifyProductListingProcessingError> {
            let count = {
                let mut calls = self.0.lock().unwrap();
                *calls += 1;
                *calls
            };
            if count == 2 {
                return Err(ShopifyProductListingProcessingError::ForwardUnconfirmed);
            }
            if count == 3 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_fail_unconfirmed_timed_out_and_unprocessed_upstream_messages_without_losing_prior_success()
     {
        let mut batch = events(vec![
            ("accepted", valid_body()),
            ("unconfirmed", valid_body()),
            ("timed-out", valid_body()),
            ("unprocessed", valid_body()),
        ]);
        batch.context.deadline = future_deadline(Duration::from_millis(1_080));
        let calls = Arc::new(Mutex::new(0));
        let response = handler(batch, &MixedBatchProcessor(Arc::clone(&calls)))
            .await
            .unwrap();
        assert_eq!(
            vec!["unconfirmed", "timed-out", "unprocessed"],
            identifiers(response)
        );
        assert_eq!(3, *calls.lock().unwrap());
    }

    fn event(message_id: &str, body: String) -> LambdaEvent<SqsEvent> {
        events(vec![(message_id, body)])
    }

    fn events(records: Vec<(&str, String)>) -> LambdaEvent<SqsEvent> {
        let records = records
            .into_iter()
            .map(|(message_id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = Some(message_id.to_owned());
                message.body = Some(body);
                message
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let mut context = Context::default();
        context.deadline = future_deadline(Duration::from_secs(60));
        LambdaEvent::new(sqs_event, context)
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

    fn future_deadline(remaining: Duration) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .saturating_add(remaining.as_millis()) as u64
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
        InvalidPayload,
        UnconfirmedOnSecondCall,
    }

    #[derive(Clone)]
    struct FakeProcessor {
        calls: Arc<Mutex<usize>>,
        result: FakeResult,
    }

    impl FakeProcessor {
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

        fn invalid_payload() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::InvalidPayload,
            }
        }

        fn unconfirmed_on_second_call() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::UnconfirmedOnSecondCall,
            }
        }
    }

    fn call_count(processor: &FakeProcessor) -> usize {
        *processor
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    #[async_trait::async_trait]
    impl ShopifyProductListingProcessorUseCase for FakeProcessor {
        async fn execute(
            &self,
            _context: &OperationContext,
            _kind: ShopifyProductEventKind,
            _shop_domain: Domain,
            _payload: Value,
            _provenance: ShopifyEventProvenance,
            _upstream_message_id: &str,
        ) -> Result<(), ShopifyProductListingProcessingError> {
            let call_count = {
                let mut calls = self.calls.lock().unwrap_or_else(|error| error.into_inner());
                *calls += 1;
                *calls
            };
            match self.result {
                FakeResult::Success => Ok(()),
                FakeResult::Failure => {
                    Err(ShopifyProductListingProcessingError::ForwardUnconfirmed)
                }
                FakeResult::InvalidPayload => {
                    Err(ShopifyProductListingProcessingError::InvalidPayload(
                        ShopifyProductEventError::MissingTitle,
                    ))
                }
                FakeResult::UnconfirmedOnSecondCall if call_count == 2 => {
                    Err(ShopifyProductListingProcessingError::ForwardUnconfirmed)
                }
                FakeResult::UnconfirmedOnSecondCall => Ok(()),
            }
        }
    }
}
