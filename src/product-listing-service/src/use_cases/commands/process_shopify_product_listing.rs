use crate::ports::{
    ProductListingRawIngestionMethod, ProductListingRawProviderReceipt,
    ProviderReceiptDeliveryIdError, ProviderReceiptScope, ProviderReceiptScopeError,
    SourceEvidenceSha256,
};
use crate::use_cases::{
    CaptureProductListingRawObservationCommand, IndexedProductListingIngestionIntent,
    ProductListingIngestionActor, ProductListingIngestionIdempotencyKey,
    ProductListingIngestionIntent, ProductListingIngestionOperation,
    ProductListingIngestionOutcome, ProductListingIngestionSubmission,
    ProductListingIngestionSubmissionError, SubmitInternalProductListingIngestionUseCase,
    product_listing_ingestion_identity,
};
use application::operation_context::{OperationContext, Principal};
use listing_source_core::Domain;
use listing_source_service::ports::{ListingSourceReadError, ShopifySourceReader};
use product_listing_normalization::{RawProductListingProvenance, SourcePayload};
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::ports::shopify_product_decoder::{
    ShopifyListingAction, ShopifyProductDecoder, ShopifyProductEventKind,
};
const SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX: &str = "shopify-webhook:";
const EVENTBRIDGE_RECEIPT_DELIVERY_ID_PREFIX: &str = "eventbridge:";
const SQS_DELIVERY_ID_PREFIX: &str = "sqs:";
const SHOPIFY_SUBMISSION_IDENTITY_DOMAIN: &[u8] = b"aura.shopify.ingestion.delivery.v1";
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
    InvalidPayload(#[source] application::error::BoxError),
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
pub trait ProcessShopifyProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        shop_domain: Domain,
        payload: SourcePayload,
        provenance: ShopifyEventProvenance,
        upstream_message_id: &str,
    ) -> Result<(), ShopifyProductListingProcessingError>;
}

pub struct ProcessShopifyProductListingHandler<S, I, D> {
    sources: S,
    intake: I,
    decoder: D,
}

impl<S, I, D> ProcessShopifyProductListingHandler<S, I, D> {
    pub fn new(sources: S, intake: I, decoder: D) -> Self {
        Self {
            sources,
            intake,
            decoder,
        }
    }
}

#[async_trait::async_trait]
impl<S, I, D> ProcessShopifyProductListingUseCase for ProcessShopifyProductListingHandler<S, I, D>
where
    S: ShopifySourceReader,
    I: SubmitInternalProductListingIngestionUseCase,
    D: ShopifyProductDecoder,
{
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        source_domain: Domain,
        payload: SourcePayload,
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
        let ShopifyListingAction::Capture(mut observation) = self
            .decoder
            .decode(kind, &source, payload)
            .map_err(ShopifyProductListingProcessingError::InvalidPayload)?
        else {
            return Ok(());
        };
        observation.source_occurred_at = provenance
            .triggered_at
            .as_deref()
            .map(|value| {
                time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
            })
            .transpose()
            .map_err(|source| {
                ShopifyProductListingProcessingError::InvalidPayload(Box::new(source))
            })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
    const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
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
}
