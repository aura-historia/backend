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
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tracing::{info, warn};

use crate::ports::shopify_product_decoder::{
    ShopifyListingAction, ShopifyProductDecoder, ShopifyProductEventKind,
};
use crate::ports::shopify_webhook_clock::ShopifyWebhookClock;
const SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX: &str = "shopify-webhook:";
const SHOPIFY_SUBMISSION_IDENTITY_DOMAIN: &[u8] = b"aura.shopify.ingestion.delivery.v1";
/// Shopify retries a failed delivery for about four hours; the margin covers a delayed first
/// attempt. An older trigger time is not a delivery Shopify still makes.
const SHOPIFY_TRIGGER_MAX_AGE: Duration = Duration::hours(6);
/// Clock skew tolerated between Shopify and this host.
const SHOPIFY_TRIGGER_MAX_LEAD: Duration = Duration::minutes(5);

/// The provider metadata of one Shopify delivery. Only the body is signed, so every field is
/// unauthenticated and bounded before it orders or identifies an observation.
#[derive(Debug, Clone)]
pub struct ShopifyEventProvenance {
    pub topic: String,
    pub shopify_event_id: Option<String>,
    pub webhook_id: String,
    pub triggered_at: Option<String>,
}

/// Why `X-Shopify-Triggered-At` cannot order an observation.
#[derive(Debug, thiserror::Error)]
pub enum ShopifyTriggeredAtError {
    #[error("Shopify trigger time is not an RFC 3339 timestamp")]
    Malformed(#[source] time::error::Parse),
    #[error("Shopify trigger time is too far in the future")]
    InFuture,
    #[error("Shopify trigger time is older than Shopify's retry horizon")]
    Expired,
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
    #[error("Shopify trigger time is invalid")]
    InvalidTriggeredAt(#[source] ShopifyTriggeredAtError),
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
    ) -> Result<(), ShopifyProductListingProcessingError>;
}

pub struct ProcessShopifyProductListingHandler<S, I, D, C> {
    sources: S,
    intake: I,
    decoder: D,
    clock: C,
}

impl<S, I, D, C> ProcessShopifyProductListingHandler<S, I, D, C> {
    pub fn new(sources: S, intake: I, decoder: D, clock: C) -> Self {
        Self {
            sources,
            intake,
            decoder,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<S, I, D, C> ProcessShopifyProductListingUseCase
    for ProcessShopifyProductListingHandler<S, I, D, C>
where
    S: ShopifySourceReader,
    I: SubmitInternalProductListingIngestionUseCase,
    D: ShopifyProductDecoder,
    C: ShopifyWebhookClock,
{
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        source_domain: Domain,
        payload: SourcePayload,
        provenance: ShopifyEventProvenance,
    ) -> Result<(), ShopifyProductListingProcessingError> {
        let Some(source) = self
            .sources
            .find_by_domain(&source_domain)
            .await
            .map_err(ShopifyProductListingProcessingError::ListingSourceLookup)?
        else {
            return Ok(());
        };
        let ShopifyListingAction::Capture(observation) = self
            .decoder
            .decode(kind, &source, payload)
            .map_err(ShopifyProductListingProcessingError::InvalidPayload)?
        else {
            return Ok(());
        };
        let source_occurred_at =
            source_occurred_at(provenance.triggered_at.as_deref(), self.clock.now_utc())
                .map_err(ShopifyProductListingProcessingError::InvalidTriggeredAt)?;
        let delivery_id = shopify_delivery_identity(&provenance.webhook_id).map_err(|error| {
            ShopifyProductListingProcessingError::InvalidProviderReceipt(
                ShopifyProviderReceiptError::DeliveryId(error),
            )
        })?;
        let provider_receipt = shopify_provider_receipt(
            provenance.topic.as_str(),
            &delivery_id,
            observation.input.source_payload(),
        )
        .map_err(ShopifyProductListingProcessingError::InvalidProviderReceipt)?;
        // `eventBridgeEventId` stays in the retained shape, always `null`, so raw provenance keeps
        // one schema across the EventBridge and HTTPS delivery eras.
        let raw_provenance = RawProductListingProvenance::new(json!({
            "topic": &provenance.topic,
            "shopifyEventId": &provenance.shopify_event_id,
            "shopifyWebhookId": &provenance.webhook_id,
            "eventBridgeEventId": null,
            "shopifyTriggeredAt": &provenance.triggered_at,
        }))
        .map_err(ShopifyProductListingProcessingError::InvalidProvenance)?;

        let idempotency_key = shopify_submission_key(&provenance.topic, &delivery_id)?;
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
                        source_occurred_at,
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

/// Bounds the unsigned trigger time: a replayed body must not carry an ordering time, such as a
/// far-future deletion barrier, that Shopify would never send.
fn source_occurred_at(
    triggered_at: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<OffsetDateTime>, ShopifyTriggeredAtError> {
    let Some(triggered_at) = triggered_at else {
        return Ok(None);
    };
    let occurred_at = OffsetDateTime::parse(triggered_at, &Rfc3339)
        .map_err(ShopifyTriggeredAtError::Malformed)?;
    if occurred_at > now + SHOPIFY_TRIGGER_MAX_LEAD {
        return Err(ShopifyTriggeredAtError::InFuture);
    }
    if occurred_at < now - SHOPIFY_TRIGGER_MAX_AGE {
        return Err(ShopifyTriggeredAtError::Expired);
    }
    Ok(Some(occurred_at))
}

fn shopify_provider_receipt(
    topic: &str,
    delivery_id: &str,
    source_payload: &SourcePayload,
) -> Result<Option<ProductListingRawProviderReceipt>, ShopifyProviderReceiptError> {
    let scope =
        ProviderReceiptScope::new(topic.to_owned()).map_err(ShopifyProviderReceiptError::Scope)?;
    let source_evidence_sha256 = source_payload
        .canonical_sha256()
        .map_err(ShopifyProviderReceiptError::SourcePayload)?;
    ProductListingRawProviderReceipt::new(
        scope,
        delivery_id.to_owned(),
        SourceEvidenceSha256::new(*source_evidence_sha256.as_bytes()),
    )
    .map(Some)
    .map_err(ShopifyProviderReceiptError::DeliveryId)
}

fn shopify_delivery_identity(webhook_id: &str) -> Result<String, ProviderReceiptDeliveryIdError> {
    if webhook_id.is_empty() {
        return Err(ProviderReceiptDeliveryIdError::Empty);
    }
    Ok(format!(
        "{SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX}{webhook_id}"
    ))
}

fn shopify_submission_key(
    topic: &str,
    delivery_id: &str,
) -> Result<ProductListingIngestionIdempotencyKey, ShopifyProductListingProcessingError> {
    let mut hasher = Sha256::new();
    for field in [
        SHOPIFY_SUBMISSION_IDENTITY_DOMAIN,
        topic.as_bytes(),
        delivery_id.as_bytes(),
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
    const SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD: &str = "product_listings/add";
    const SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE: &str = "product_listings/update";

    fn at(value: &str) -> OffsetDateTime {
        OffsetDateTime::parse(value, &Rfc3339).unwrap()
    }

    #[test]
    fn should_namespace_delivery_identity_and_reject_empty_webhook_id() {
        assert_eq!(
            "shopify-webhook:delivery-1",
            shopify_delivery_identity("delivery-1").unwrap()
        );
        assert!(matches!(
            shopify_delivery_identity(""),
            Err(ProviderReceiptDeliveryIdError::Empty)
        ));
    }

    #[test]
    fn should_scope_submission_key_to_topic_and_delivery() {
        let key = |topic: &str, webhook: &str| {
            shopify_submission_key(topic, &shopify_delivery_identity(webhook).unwrap()).unwrap()
        };
        assert_eq!(
            key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, "webhook"),
            key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, "webhook")
        );
        assert_ne!(
            key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, "webhook"),
            key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE, "webhook")
        );
        assert_ne!(
            key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, "webhook"),
            key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, "other-webhook")
        );
        let delivery_key = key(SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, "webhook");
        let (_, first_command) = product_listing_ingestion_identity(
            &ProductListingIngestionActor::System,
            listing_source_core::ListingSourceId::new(),
            &delivery_key,
            ProductListingIngestionOperation::CaptureRaw,
            0,
        );
        let (_, second_command) = product_listing_ingestion_identity(
            &ProductListingIngestionActor::System,
            listing_source_core::ListingSourceId::new(),
            &delivery_key,
            ProductListingIngestionOperation::CaptureRaw,
            0,
        );
        assert_ne!(first_command, second_command);
    }

    #[test]
    fn should_accept_trigger_time_within_retry_horizon_and_clock_skew() {
        let now = at("2026-11-20T12:00:00Z");
        for triggered_at in [
            "2026-11-20T12:00:00Z",
            "2026-11-20T12:05:00Z",
            "2026-11-20T06:00:00Z",
            "2026-11-20T13:00:00+01:00",
        ] {
            assert_eq!(
                Some(at(triggered_at)),
                source_occurred_at(Some(triggered_at), now).unwrap(),
                "{triggered_at}"
            );
        }
        assert_eq!(None, source_occurred_at(None, now).unwrap());
    }

    #[test]
    fn should_reject_trigger_time_outside_retry_horizon_and_clock_skew() {
        let now = at("2026-11-20T12:00:00Z");
        assert!(matches!(
            source_occurred_at(Some("2026-11-20T12:05:01Z"), now),
            Err(ShopifyTriggeredAtError::InFuture)
        ));
        assert!(matches!(
            source_occurred_at(Some("9999-12-31T23:59:59Z"), now),
            Err(ShopifyTriggeredAtError::InFuture)
        ));
        assert!(matches!(
            source_occurred_at(Some("2026-11-20T05:59:59Z"), now),
            Err(ShopifyTriggeredAtError::Expired)
        ));
        assert!(matches!(
            source_occurred_at(Some("yesterday"), now),
            Err(ShopifyTriggeredAtError::Malformed(_))
        ));
    }
}
