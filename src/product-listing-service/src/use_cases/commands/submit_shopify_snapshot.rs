use crate::ports::ProductListingRawIngestionMethod;
use crate::ports::shopify_product_decoder::{
    ShopifyListingAction, ShopifyProductDecoder, ShopifyProductEventKind,
};
use crate::use_cases::{
    CaptureProductListingRawObservationCommand, IndexedProductListingIngestionIntent,
    ProductListingIngestionIdempotencyKey, ProductListingIngestionIntent,
    ProductListingIngestionSubmission, ProductListingIngestionSubmissionResult,
    SubmitPartnerProductListingIngestionUseCase,
};
use application::operation_context::{CredentialCapability, OperationContext};
use listing_source_core::{Domain, ListingSourceId};
use listing_source_service::ports::{ShopifySource, ShopifySourceReader};
use product_listing_normalization::{RawProductListingProvenance, SourcePayload};
use time::OffsetDateTime;

pub struct SubmitShopifySnapshotCommand {
    pub listing_source_id: ListingSourceId,
    pub domain: Domain,
    pub snapshot_started_at: OffsetDateTime,
    pub selection_complete: bool,
    pub idempotency_key: ProductListingIngestionIdempotencyKey,
    pub items: Vec<ShopifySnapshotItem>,
}

pub struct ShopifySnapshotItem {
    pub kind: ShopifyProductEventKind,
    pub payload: SourcePayload,
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitShopifySnapshotError {
    #[error("snapshot authentication or capability is missing")]
    Forbidden,
    #[error("Shopify source is absent or does not match the requested source")]
    SourceMismatch,
    #[error("a complete selection and one to one hundred snapshot items are required")]
    IncompleteSelection,
    #[error("Shopify source lookup failed")]
    SourceLookup(#[source] listing_source_service::ports::ListingSourceReadError),
    #[error("Shopify snapshot is invalid")]
    InvalidPayload(#[source] application::error::BoxError),
    #[error("Shopify snapshot admission failed")]
    Submission(#[source] crate::use_cases::ProductListingIngestionSubmissionError),
}

#[async_trait::async_trait]
pub trait SubmitShopifySnapshotUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitShopifySnapshotCommand,
    ) -> Result<ProductListingIngestionSubmissionResult, SubmitShopifySnapshotError>;
}

pub struct SubmitShopifySnapshotHandler<S, I, D> {
    sources: S,
    intake: I,
    decoder: D,
}

impl<S, I, D> SubmitShopifySnapshotHandler<S, I, D> {
    pub fn new(sources: S, intake: I, decoder: D) -> Self {
        Self {
            sources,
            intake,
            decoder,
        }
    }
}

#[async_trait::async_trait]
impl<
    S: ShopifySourceReader,
    I: SubmitPartnerProductListingIngestionUseCase,
    D: ShopifyProductDecoder,
> SubmitShopifySnapshotUseCase for SubmitShopifySnapshotHandler<S, I, D>
{
    #[tracing::instrument(skip_all, fields(listing_source_id = %command.listing_source_id, request_id = %context.request_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitShopifySnapshotCommand,
    ) -> Result<ProductListingIngestionSubmissionResult, SubmitShopifySnapshotError> {
        context
            .require_credential_capability(CredentialCapability::ProductListingsWrite)
            .map_err(|_| SubmitShopifySnapshotError::Forbidden)?;
        if !command.selection_complete || command.items.is_empty() || command.items.len() > 100 {
            return Err(SubmitShopifySnapshotError::IncompleteSelection);
        }
        let source = self
            .sources
            .find_by_domain(&command.domain)
            .await
            .map_err(SubmitShopifySnapshotError::SourceLookup)?
            .filter(|source| source.listing_source_id == command.listing_source_id)
            .ok_or(SubmitShopifySnapshotError::SourceMismatch)?;
        let submission = prepare_submission(&self.decoder, &source, command)?;
        self.intake
            .execute(context, submission)
            .await
            .map_err(SubmitShopifySnapshotError::Submission)
    }
}

fn prepare_submission(
    decoder: &impl ShopifyProductDecoder,
    source: &ShopifySource,
    command: SubmitShopifySnapshotCommand,
) -> Result<ProductListingIngestionSubmission, SubmitShopifySnapshotError> {
    let mut items = Vec::with_capacity(command.items.len());
    for (index, item) in command.items.into_iter().enumerate() {
        let ShopifyListingAction::Capture(observation) = decoder
            .decode(item.kind, source, item.payload)
            .map_err(SubmitShopifySnapshotError::InvalidPayload)?
        else {
            return Err(SubmitShopifySnapshotError::IncompleteSelection);
        };
        let provenance = RawProductListingProvenance::new(serde_json::json!({
            "shopifySnapshotStartedAt": command.snapshot_started_at.format(&time::format_description::well_known::Rfc3339)
                .map_err(|error| SubmitShopifySnapshotError::InvalidPayload(Box::new(error)))?,
            "selectionComplete": true,
        })).map_err(|error| SubmitShopifySnapshotError::InvalidPayload(Box::new(error)))?;
        items.push(IndexedProductListingIngestionIntent {
            index,
            intent: ProductListingIngestionIntent::CaptureRaw(
                CaptureProductListingRawObservationCommand {
                    listing_source_id: command.listing_source_id,
                    ingestion_method: ProductListingRawIngestionMethod::Shopify,
                    source_record_key: observation.source_record_key,
                    input: observation.input,
                    provenance,
                    source_event_id: None,
                    source_occurred_at: Some(command.snapshot_started_at),
                    provider_receipt: None,
                },
            ),
        });
    }
    Ok(ProductListingIngestionSubmission {
        listing_source_id: command.listing_source_id,
        original_input_count: items.len(),
        idempotency_key: Some(command.idempotency_key),
        items,
    })
}
