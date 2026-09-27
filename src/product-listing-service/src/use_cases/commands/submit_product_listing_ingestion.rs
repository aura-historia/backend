use super::product_listing_ingestion::{
    ProductListingIngestionActor, ProductListingIngestionCommandId,
    ProductListingIngestionIdempotencyKey, ProductListingIngestionIntent,
    ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
    ProductListingIngestionMetadata, ProductListingIngestionOperation,
    ProductListingIngestionOutcome, ProductListingIngestionRejectionReason,
    ProductListingIngestionSubmissionId,
};
use crate::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    product_listing_auction_patch::validate_product_listing_auction_patch,
    use_cases::commands::capture_product_listing_raw_observation::{
        SourceRecordKeyValidationError, validate_source_record_key_value,
    },
};
use application::operation_context::{
    CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
};
use listing_source_core::ListingSourceId;

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const SUBMISSION_IDENTITY_DOMAIN: &[u8] = b"aura.product-listing-ingestion.submission.v1";
const COMMAND_IDENTITY_DOMAIN: &[u8] = b"aura.product-listing-ingestion.command.v1";

/// One typed item with its position in the unfiltered original request.
/// One typed item with its position in the unfiltered original request.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedProductListingIngestionIntent {
    pub index: usize,
    pub intent: ProductListingIngestionIntent,
}

/// One source-scoped submission. `items` may be sparse after transport parsing failures.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingIngestionSubmission {
    pub listing_source_id: ListingSourceId,
    pub original_input_count: usize,
    pub idempotency_key: Option<ProductListingIngestionIdempotencyKey>,
    pub items: Vec<IndexedProductListingIngestionIntent>,
}

/// Admission result for one source-scoped batch, including every typed item's disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingIngestionSubmissionResult {
    pub submission_id: ProductListingIngestionSubmissionId,
    /// Echo this effective key from REST. A generated value cannot be recovered after a lost reply.
    pub idempotency_key: ProductListingIngestionIdempotencyKey,
    pub original_input_count: usize,
    /// Contains service-known outcomes for typed items; transport parse failures remain API-owned.
    pub items: Vec<ProductListingIngestionItemOutcome>,
}

impl ProductListingIngestionSubmissionResult {
    /// Counts only confirmed queue acceptance, never uncertain or unattempted sends.
    pub fn confirmed_accepted_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item.outcome, ProductListingIngestionOutcome::Accepted))
            .count()
    }
}

/// Whole-call failures that occur before publication can produce per-item outcomes.
#[derive(Debug, thiserror::Error)]
pub enum ProductListingIngestionSubmissionError {
    #[error("authenticated principal required")]
    AuthenticationRequired,
    #[error("principal is not permitted to submit product listing ingestion")]
    Forbidden,
    #[error("submission indices must be unique and within the original input count")]
    InconsistentInputIndices,
    #[error("product listing ingestion publisher could not start the submission")]
    PublisherNotStarted {
        #[source]
        source: ProductListingIngestionPublishError,
    },
}

/// Partner intake contract for authenticated users and delegated writers.
#[async_trait::async_trait]
pub trait SubmitPartnerProductListingIngestionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        submission: ProductListingIngestionSubmission,
    ) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError>;
}

/// Internal intake contract for trusted Service and System principals.
#[async_trait::async_trait]
pub trait SubmitInternalProductListingIngestionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        submission: ProductListingIngestionSubmission,
    ) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError>;
}

/// Partner-facing intake: accepts users and delegated users with ProductListingsWrite capability.
pub struct SubmitPartnerProductListingIngestionHandler<P> {
    publisher: P,
}

impl<P> SubmitPartnerProductListingIngestionHandler<P> {
    pub fn new(publisher: P) -> Self {
        Self { publisher }
    }
}

#[async_trait::async_trait]
impl<P> SubmitPartnerProductListingIngestionUseCase
    for SubmitPartnerProductListingIngestionHandler<P>
where
    P: ProductListingIngestionPublisher,
{
    async fn execute(
        &self,
        context: &OperationContext,
        submission: ProductListingIngestionSubmission,
    ) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError>
    {
        let actor = authorize_partner(context)?;
        submit(&self.publisher, context, actor, submission).await
    }
}

/// Internal intake: accepts only trusted Service or System contexts and has no database dependency.
pub struct SubmitInternalProductListingIngestionHandler<P> {
    publisher: P,
}

impl<P> SubmitInternalProductListingIngestionHandler<P> {
    pub fn new(publisher: P) -> Self {
        Self { publisher }
    }
}

#[async_trait::async_trait]
impl<P> SubmitInternalProductListingIngestionUseCase
    for SubmitInternalProductListingIngestionHandler<P>
where
    P: ProductListingIngestionPublisher,
{
    async fn execute(
        &self,
        context: &OperationContext,
        submission: ProductListingIngestionSubmission,
    ) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError>
    {
        let actor = authorize_internal(context)?;
        submit(&self.publisher, context, actor, submission).await
    }
}

fn authorize_partner(
    context: &OperationContext,
) -> Result<ProductListingIngestionActor, ProductListingIngestionSubmissionError> {
    match &context.principal {
        Principal::Anonymous => Err(ProductListingIngestionSubmissionError::AuthenticationRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            context
                .require_credential_capability(CredentialCapability::ProductListingsWrite)
                .map_err(map_credential_authorization)?;
            ProductListingIngestionActor::from_principal(&context.principal)
                .ok_or(ProductListingIngestionSubmissionError::AuthenticationRequired)
        }
        Principal::Service(_) | Principal::System => {
            Err(ProductListingIngestionSubmissionError::Forbidden)
        }
    }
}

fn authorize_internal(
    context: &OperationContext,
) -> Result<ProductListingIngestionActor, ProductListingIngestionSubmissionError> {
    match &context.principal {
        Principal::Anonymous => Err(ProductListingIngestionSubmissionError::AuthenticationRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            Err(ProductListingIngestionSubmissionError::Forbidden)
        }
        Principal::Service(_) | Principal::System => {
            ProductListingIngestionActor::from_principal(&context.principal)
                .ok_or(ProductListingIngestionSubmissionError::AuthenticationRequired)
        }
    }
}

fn map_credential_authorization(
    error: CredentialAuthorizationError,
) -> ProductListingIngestionSubmissionError {
    match error {
        CredentialAuthorizationError::AuthenticationRequired(_) => {
            ProductListingIngestionSubmissionError::AuthenticationRequired
        }
        CredentialAuthorizationError::InsufficientCapability { .. } => {
            ProductListingIngestionSubmissionError::Forbidden
        }
    }
}

async fn submit<P: ProductListingIngestionPublisher>(
    publisher: &P,
    context: &OperationContext,
    actor: ProductListingIngestionActor,
    mut submission: ProductListingIngestionSubmission,
) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError> {
    validate_indices(&submission)?;

    let idempotency_key = submission
        .idempotency_key
        .take()
        .unwrap_or_else(ProductListingIngestionIdempotencyKey::generated);
    let identity_seed =
        IngestionIdentitySeed::new(&actor, submission.listing_source_id, &idempotency_key);
    let submission_id = identity_seed.submission_id();

    submission.items.sort_by_key(|item| item.index);
    let mut outcomes = Vec::with_capacity(submission.items.len());
    let mut prepared = Vec::with_capacity(submission.items.len());

    for item in submission.items {
        let operation = item.intent.operation();
        let command_id = identity_seed.command_id(operation, item.index);
        let metadata = ProductListingIngestionMetadata {
            submission_id: submission_id.clone(),
            command_id: command_id.clone(),
            index: item.index,
            input_count: submission.original_input_count,
            listing_source_id: submission.listing_source_id,
            operation,
            actor: actor.clone(),
            request_id: context.request_id.clone(),
            correlation_id: context.correlation_id.clone(),
        };

        if let Err(reason) = validate_intent(submission.listing_source_id, &item.intent) {
            outcomes.push(ProductListingIngestionItemOutcome {
                index: item.index,
                command_id,
                outcome: ProductListingIngestionOutcome::Rejected {
                    reason,
                    retryable: false,
                },
            });
        } else {
            prepared.push(ProductListingIngestionMessage {
                metadata,
                intent: item.intent,
            });
        }
    }

    if !prepared.is_empty() {
        let expected: Vec<_> = prepared
            .iter()
            .map(|message| (message.metadata.index, message.metadata.command_id.clone()))
            .collect();
        let published = publisher.publish(prepared).await.map_err(|source| {
            ProductListingIngestionSubmissionError::PublisherNotStarted { source }
        })?;
        outcomes.extend(reconcile_publisher_outcomes(&expected, published));
    }

    outcomes.sort_by_key(|outcome| outcome.index);
    Ok(ProductListingIngestionSubmissionResult {
        submission_id,
        idempotency_key,
        original_input_count: submission.original_input_count,
        items: outcomes,
    })
}

fn validate_indices(
    submission: &ProductListingIngestionSubmission,
) -> Result<(), ProductListingIngestionSubmissionError> {
    let mut seen = HashSet::with_capacity(submission.items.len());
    for item in &submission.items {
        if item.index >= submission.original_input_count || !seen.insert(item.index) {
            return Err(ProductListingIngestionSubmissionError::InconsistentInputIndices);
        }
    }
    Ok(())
}

fn validate_intent(
    listing_source_id: ListingSourceId,
    intent: &ProductListingIngestionIntent,
) -> Result<(), ProductListingIngestionRejectionReason> {
    if intent.listing_source_id() != listing_source_id {
        return Err(ProductListingIngestionRejectionReason::ListingSourceMismatch);
    }

    let invalid_auction_patch = |patch| {
        validate_product_listing_auction_patch(None, patch)
            .err()
            .map(|_| ProductListingIngestionRejectionReason::InvalidAuctionPatch)
    };
    match intent {
        ProductListingIngestionIntent::Create(command) => {
            if command
                .auction
                .as_ref()
                .and_then(invalid_auction_patch)
                .is_some()
            {
                return Err(ProductListingIngestionRejectionReason::InvalidAuctionPatch);
            }
        }
        ProductListingIngestionIntent::Update { command, .. } => {
            if matches!(command.url, application::patch_field::PatchField::Clear) {
                return Err(ProductListingIngestionRejectionReason::UrlCannotBeCleared);
            }
            if let application::patch_field::PatchField::Set(patch) = &command.auction
                && invalid_auction_patch(patch).is_some()
            {
                return Err(ProductListingIngestionRejectionReason::InvalidAuctionPatch);
            }
        }
        ProductListingIngestionIntent::Upsert(command) => {
            if let application::patch_field::PatchField::Set(patch) = &command.auction
                && invalid_auction_patch(patch).is_some()
            {
                return Err(ProductListingIngestionRejectionReason::InvalidAuctionPatch);
            }
        }
        ProductListingIngestionIntent::Withdraw(_) => {}
        ProductListingIngestionIntent::CaptureRaw(command) => {
            validate_source_record_key_value(&command.source_record_key).map_err(|error| {
                match error {
                    SourceRecordKeyValidationError::TooLong { len, max } => {
                        ProductListingIngestionRejectionReason::SourceRecordKeyTooLong { len, max }
                    }
                    SourceRecordKeyValidationError::EmbeddedNul => {
                        ProductListingIngestionRejectionReason::SourceRecordKeyEmbeddedNul
                    }
                }
            })?;
            command
                .input
                .hash()
                .map_err(|_| ProductListingIngestionRejectionReason::InvalidNormalizationInput)?;
        }
    }
    Ok(())
}

fn reconcile_publisher_outcomes(
    expected: &[(usize, ProductListingIngestionCommandId)],
    returned: Vec<ProductListingIngestionItemOutcome>,
) -> Vec<ProductListingIngestionItemOutcome> {
    let expected_by_id: HashMap<_, _> = expected
        .iter()
        .map(|(index, command_id)| (command_id.clone(), *index))
        .collect();
    let mut returned_by_id: HashMap<ProductListingIngestionCommandId, Vec<_>> = HashMap::new();

    for outcome in returned {
        if expected_by_id.get(&outcome.command_id) == Some(&outcome.index) {
            returned_by_id
                .entry(outcome.command_id.clone())
                .or_default()
                .push(outcome);
        }
    }

    expected
        .iter()
        .map(
            |(index, command_id)| match returned_by_id.remove(command_id) {
                Some(mut matching) if matching.len() == 1 => matching
                    .pop()
                    .unwrap_or_else(|| unconfirmed(*index, command_id.clone())),
                _ => unconfirmed(*index, command_id.clone()),
            },
        )
        .collect()
}

fn unconfirmed(
    index: usize,
    command_id: ProductListingIngestionCommandId,
) -> ProductListingIngestionItemOutcome {
    ProductListingIngestionItemOutcome {
        index,
        command_id,
        outcome: ProductListingIngestionOutcome::Unconfirmed,
    }
}

struct IngestionIdentitySeed(Vec<u8>);

impl IngestionIdentitySeed {
    fn new(
        actor: &ProductListingIngestionActor,
        listing_source_id: ListingSourceId,
        idempotency_key: &ProductListingIngestionIdempotencyKey,
    ) -> Self {
        let actor_id = actor.identity_id().unwrap_or_default();
        let listing_source_id = listing_source_id.to_string();
        let mut seed = Vec::new();
        append_length_prefixed(&mut seed, actor.identity_kind().as_bytes());
        append_length_prefixed(&mut seed, actor_id.as_bytes());
        append_length_prefixed(&mut seed, listing_source_id.as_bytes());
        append_length_prefixed(&mut seed, idempotency_key.as_str().as_bytes());
        Self(seed)
    }

    fn submission_id(&self) -> ProductListingIngestionSubmissionId {
        let mut hasher = Sha256::new();
        append_length_prefixed_hash(&mut hasher, SUBMISSION_IDENTITY_DOMAIN);
        append_length_prefixed_hash(&mut hasher, &self.0);
        ProductListingIngestionSubmissionId::from_digest(hasher.finalize().into())
    }

    fn command_id(
        &self,
        operation: ProductListingIngestionOperation,
        index: usize,
    ) -> ProductListingIngestionCommandId {
        let mut hasher = Sha256::new();
        append_length_prefixed_hash(&mut hasher, COMMAND_IDENTITY_DOMAIN);
        append_length_prefixed_hash(&mut hasher, &self.0);
        append_length_prefixed_hash(&mut hasher, operation.as_str().as_bytes());
        append_length_prefixed_hash(&mut hasher, &(index as u64).to_be_bytes());
        ProductListingIngestionCommandId::from_digest(hasher.finalize().into())
    }
}

fn append_length_prefixed(output: &mut Vec<u8>, field: &[u8]) {
    output.extend_from_slice(&(field.len() as u64).to_be_bytes());
    output.extend_from_slice(field);
}

fn append_length_prefixed_hash(hasher: &mut Sha256, field: &[u8]) {
    hasher.update((field.len() as u64).to_be_bytes());
    hasher.update(field);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::commands::product_listing_ingestion::ProductListingIngestionNotAttemptedReason;
    use crate::{
        ports::ProductListingIngestionPublisher,
        ports::{ProductListingRawProviderReceipt, ProviderReceiptScope, SourceEvidenceSha256},
        use_cases::{
            CaptureProductListingRawObservationCommand, CreateProductListingCommand,
            UpdateProductListingCommand, UpsertProductListingCommand,
        },
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        patch_field::PatchField,
    };
    use async_trait::async_trait;
    use indexmap::IndexSet;
    use localization::{Language, Localized};

    use product_listing_core::{
        description::Description, product_listing::ProductListingPricing,
        product_listing_id::ProductListingKey, product_listing_image::ProductListingImage,
        source_listing_id::SourceListingId, title::Title,
    };
    use product_listing_normalization::{
        NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
        RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
        SourcePayload,
    };
    use std::sync::{Arc, Mutex};
    use url::Url;
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct FakePublisher {
        calls: Arc<Mutex<Vec<Vec<ProductListingIngestionMessage>>>>,
        outcomes: Vec<ProductListingIngestionOutcome>,
    }

    #[async_trait]
    impl ProductListingIngestionPublisher for FakePublisher {
        async fn publish(
            &self,
            commands: Vec<ProductListingIngestionMessage>,
        ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>
        {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(commands.clone());
            Ok(commands
                .into_iter()
                .zip(
                    self.outcomes
                        .iter()
                        .cloned()
                        .chain(std::iter::repeat(ProductListingIngestionOutcome::Accepted)),
                )
                .map(|(command, outcome)| ProductListingIngestionItemOutcome {
                    index: command.metadata.index,
                    command_id: command.metadata.command_id,
                    outcome,
                })
                .collect())
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request-1"),
            correlation_id: CorrelationId::new("correlation-1"),
        }
    }

    fn source_listing_id(value: &str) -> SourceListingId {
        SourceListingId::try_from(value)
            .unwrap_or_else(|error| panic!("valid SourceListingId: {error}"))
    }

    fn submission(
        listing_source_id: ListingSourceId,
        items: Vec<IndexedProductListingIngestionIntent>,
        original_input_count: usize,
        key: &str,
    ) -> ProductListingIngestionSubmission {
        ProductListingIngestionSubmission {
            listing_source_id,
            original_input_count,
            idempotency_key: Some(
                ProductListingIngestionIdempotencyKey::new(key)
                    .unwrap_or_else(|error| panic!("valid idempotency key: {error}")),
            ),
            items,
        }
    }

    fn upsert(
        source_id: ListingSourceId,
        source_listing_key: &str,
    ) -> ProductListingIngestionIntent {
        ProductListingIngestionIntent::Upsert(UpsertProductListingCommand {
            listing_source_id: source_id,
            source_listing_id: source_listing_id(source_listing_key),
            title: None,
            description: None,
            price: PatchField::Unchanged,
            price_estimate_min: PatchField::Unchanged,
            price_estimate_max: PatchField::Unchanged,
            availability: PatchField::Unchanged,
            url: None,
            images: PatchField::Unchanged,
            auction: PatchField::Unchanged,
        })
    }

    fn create(source_id: ListingSourceId) -> ProductListingIngestionIntent {
        ProductListingIngestionIntent::Create(CreateProductListingCommand {
            listing_source_id: source_id,
            source_listing_id: source_listing_id("source-listing"),
            title: Some(Localized {
                localization: Language::En,
                payload: Title::from("A product title"),
            }),
            description: Some(Localized {
                localization: Language::En,
                payload: Description::from("Product description"),
            }),
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://example.com/listing")
                .unwrap_or_else(|error| panic!("valid URL: {error}")),
            images: IndexSet::<ProductListingImage>::new(),
            auction: None,
        })
    }

    fn raw_capture(
        source_id: ListingSourceId,
        source_record_key: &str,
    ) -> ProductListingIngestionIntent {
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            1,
            SourcePayload::new(serde_json::json!({ "id": 1 }))
                .unwrap_or_else(|error| panic!("source payload: {error}")),
            RawProductListingValues::new(serde_json::json!({ "priceFormat": "DISPLAY_TEXT" }))
                .unwrap_or_else(|error| panic!("raw values: {error}")),
            NormalizationContext::new(serde_json::json!({ "baseUrl": "https://example.com" }))
                .unwrap_or_else(|error| panic!("normalization context: {error}")),
        )
        .unwrap_or_else(|error| panic!("normalization input: {error}"));
        let provenance = RawProductListingProvenance::new(serde_json::json!({ "receipt": "kept" }))
            .unwrap_or_else(|error| panic!("provenance: {error}"));
        ProductListingIngestionIntent::CaptureRaw(CaptureProductListingRawObservationCommand {
            listing_source_id: source_id,
            ingestion_method: crate::ports::ProductListingRawIngestionMethod::Shopify,
            source_record_key: source_record_key.to_owned(),
            input,
            provenance,
            source_event_id: Some("provider-event-1".to_owned()),
            source_occurred_at: Some(time::OffsetDateTime::UNIX_EPOCH),
            provider_receipt: Some(
                ProductListingRawProviderReceipt::new(
                    ProviderReceiptScope::new("shopify-webhook".to_owned())
                        .unwrap_or_else(|error| panic!("provider receipt scope: {error}")),
                    "delivery-1".to_owned(),
                    SourceEvidenceSha256::new([7; 32]),
                )
                .unwrap_or_else(|error| panic!("provider receipt: {error}")),
            ),
        })
    }

    fn captured_calls(publisher: &FakePublisher) -> Vec<Vec<ProductListingIngestionMessage>> {
        publisher
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[tokio::test]
    async fn partner_accepts_users_and_delegated_users_with_write_capability() {
        let source_id = ListingSourceId::new();
        let user = context(Principal::User(UserId::new()));
        let publisher = FakePublisher::default();
        let handler = SubmitPartnerProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &user,
                submission(
                    source_id,
                    vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: upsert(source_id, "source-1"),
                    }],
                    1,
                    "key-1",
                ),
            )
            .await;
        assert!(result.is_ok());

        let delegated = context(Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: [CredentialCapability::ProductListingsWrite]
                .into_iter()
                .collect(),
        });
        let delegated_handler =
            SubmitPartnerProductListingIngestionHandler::new(FakePublisher::default());
        assert!(
            delegated_handler
                .execute(&delegated, submission(source_id, vec![], 0, "key-2"))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn denied_partner_and_internal_calls_never_publish_including_empty_submissions() {
        for principal in [
            Principal::Anonymous,
            Principal::Service("crawler".to_owned()),
            Principal::System,
            Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: Default::default(),
            },
        ] {
            let publisher = FakePublisher::default();
            let handler = SubmitPartnerProductListingIngestionHandler::new(publisher);
            assert!(
                handler
                    .execute(
                        &context(principal),
                        submission(ListingSourceId::new(), vec![], 0, "key"),
                    )
                    .await
                    .is_err()
            );
            assert!(captured_calls(&handler.publisher).is_empty());
        }

        for principal in [
            Principal::Anonymous,
            Principal::User(UserId::new()),
            Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: [CredentialCapability::ProductListingsWrite]
                    .into_iter()
                    .collect(),
            },
        ] {
            let publisher = FakePublisher::default();
            let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
            assert!(
                handler
                    .execute(
                        &context(principal),
                        submission(ListingSourceId::new(), vec![], 0, "key"),
                    )
                    .await
                    .is_err()
            );
            assert!(captured_calls(&handler.publisher).is_empty());
        }
    }

    #[tokio::test]
    async fn denied_partner_and_internal_calls_do_not_publish_all_invalid_items() {
        let source_id = ListingSourceId::new();
        let invalid_intent = || ProductListingIngestionIntent::Update {
            product_key: ProductListingKey::new(source_id, source_listing_id("invalid-update")),
            command: UpdateProductListingCommand {
                url: PatchField::Clear,
                ..Default::default()
            },
        };
        let partner_publisher = FakePublisher::default();
        let partner_handler = SubmitPartnerProductListingIngestionHandler::new(partner_publisher);
        assert!(
            partner_handler
                .execute(
                    &context(Principal::DelegatedUser {
                        user_id: UserId::new(),
                        capabilities: Default::default(),
                    }),
                    submission(
                        source_id,
                        vec![IndexedProductListingIngestionIntent {
                            index: 0,
                            intent: invalid_intent(),
                        }],
                        1,
                        "key",
                    ),
                )
                .await
                .is_err()
        );
        assert!(captured_calls(&partner_handler.publisher).is_empty());

        let internal_publisher = FakePublisher::default();
        let internal_handler =
            SubmitInternalProductListingIngestionHandler::new(internal_publisher);
        assert!(
            internal_handler
                .execute(
                    &context(Principal::User(UserId::new())),
                    submission(
                        source_id,
                        vec![IndexedProductListingIngestionIntent {
                            index: 0,
                            intent: invalid_intent(),
                        }],
                        1,
                        "key",
                    ),
                )
                .await
                .is_err()
        );
        assert!(captured_calls(&internal_handler.publisher).is_empty());
    }

    #[tokio::test]
    async fn internal_submission_needs_only_a_fake_publisher_and_preserves_service_actor() {
        let source_id = ListingSourceId::new();
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::Service("shopify".to_owned())),
                submission(
                    source_id,
                    vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: upsert(source_id, "missing-target-is-allowed"),
                    }],
                    1,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("internal submission: {error}"));
        assert!(matches!(
            result.items[0].outcome,
            ProductListingIngestionOutcome::Accepted
        ));
        assert_eq!(1, result.confirmed_accepted_count());
        let calls = captured_calls(&handler.publisher);
        assert_eq!(
            calls[0][0].metadata.actor,
            ProductListingIngestionActor::Service("shopify".into())
        );
    }

    #[tokio::test]
    async fn update_and_conflicting_create_intents_reach_publisher_without_state_reads() {
        let source_id = ListingSourceId::new();
        let update_key = ProductListingKey::new(source_id, source_listing_id("missing-update"));
        let create_command = match create(source_id) {
            ProductListingIngestionIntent::Create(command) => command,
            _ => unreachable!(),
        };
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(
                    source_id,
                    vec![
                        IndexedProductListingIngestionIntent {
                            index: 0,
                            intent: ProductListingIngestionIntent::Update {
                                product_key: update_key,
                                command: UpdateProductListingCommand::default(),
                            },
                        },
                        IndexedProductListingIngestionIntent {
                            index: 1,
                            intent: ProductListingIngestionIntent::Create(create_command),
                        },
                    ],
                    2,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("submission does not read listing state: {error}"));

        assert_eq!(2, result.confirmed_accepted_count());
        let calls = captured_calls(&handler.publisher);
        assert_eq!(2, calls[0].len());
        assert!(matches!(
            calls[0][0].intent,
            ProductListingIngestionIntent::Update { .. }
        ));
        assert!(matches!(
            calls[0][1].intent,
            ProductListingIngestionIntent::Create(_)
        ));
    }

    #[tokio::test]
    async fn empty_submission_checks_authority_and_returns_zero_without_publishing() {
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(ListingSourceId::new(), vec![], 0, "key"),
            )
            .await
            .unwrap_or_else(|error| panic!("empty submission: {error}"));
        assert_eq!(0, result.original_input_count);
        assert!(result.items.is_empty());
        assert!(captured_calls(&handler.publisher).is_empty());
    }

    #[tokio::test]
    async fn missing_idempotency_key_is_generated_and_returned_even_for_empty_input() {
        let source_id = ListingSourceId::new();
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let mut request = submission(source_id, vec![], 0, "temporary-key");
        request.idempotency_key = None;

        let result = handler
            .execute(&context(Principal::System), request)
            .await
            .unwrap_or_else(|error| panic!("generated-key submission: {error}"));

        assert!(result.idempotency_key.as_str().starts_with("generated-"));
        assert!(
            ProductListingIngestionIdempotencyKey::new(result.idempotency_key.as_str()).is_ok()
        );
        assert!(captured_calls(&handler.publisher).is_empty());
    }

    #[tokio::test]
    async fn sparse_indices_are_preserved_and_invalid_final_item_does_not_block_valid_siblings() {
        let source_id = ListingSourceId::new();
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(
                    source_id,
                    vec![
                        IndexedProductListingIngestionIntent {
                            index: 2,
                            intent: raw_capture(source_id, &format!("bad{}", "x".repeat(4094))),
                        },
                        IndexedProductListingIngestionIntent {
                            index: 0,
                            intent: upsert(source_id, "source-0"),
                        },
                    ],
                    3,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("partial submission: {error}"));
        assert_eq!(
            vec![0, 2],
            result
                .items
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>()
        );
        assert!(matches!(
            result.items[0].outcome,
            ProductListingIngestionOutcome::Accepted
        ));
        assert!(matches!(
            result.items[1].outcome,
            ProductListingIngestionOutcome::Rejected {
                reason: ProductListingIngestionRejectionReason::SourceRecordKeyTooLong { .. },
                retryable: false,
            }
        ));
        let calls = captured_calls(&handler.publisher);
        assert_eq!(1, calls.len());
        assert_eq!(
            vec![0],
            calls[0]
                .iter()
                .map(|item| item.metadata.index)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn all_invalid_items_are_rejected_without_calling_publisher() {
        let source_id = ListingSourceId::new();
        let invalid = ProductListingIngestionIntent::Update {
            product_key: ProductListingKey::new(source_id, source_listing_id("bad-url")),
            command: UpdateProductListingCommand {
                url: PatchField::Clear,
                ..Default::default()
            },
        };
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(
                    source_id,
                    vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: invalid,
                    }],
                    1,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("all-invalid submission: {error}"));
        assert!(matches!(
            result.items[0].outcome,
            ProductListingIngestionOutcome::Rejected {
                reason: ProductListingIngestionRejectionReason::UrlCannotBeCleared,
                retryable: false,
            }
        ));
        assert!(captured_calls(&handler.publisher).is_empty());
    }

    #[tokio::test]
    async fn unchanged_whole_batch_retries_keep_ids_and_operation_index_scope_is_distinct() {
        let source_id = ListingSourceId::new();
        let system = context(Principal::System);
        let key = "same-key";
        let first_handler =
            SubmitInternalProductListingIngestionHandler::new(FakePublisher::default());
        let second_handler =
            SubmitInternalProductListingIngestionHandler::new(FakePublisher::default());
        let request = || {
            submission(
                source_id,
                vec![
                    IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: upsert(source_id, "source-0"),
                    },
                    IndexedProductListingIngestionIntent {
                        index: 2,
                        intent: upsert(source_id, "source-2"),
                    },
                ],
                3,
                key,
            )
        };
        let first = first_handler
            .execute(&system, request())
            .await
            .unwrap_or_else(|error| panic!("first submission: {error}"));
        let retry = second_handler
            .execute(&system, request())
            .await
            .unwrap_or_else(|error| panic!("retry submission: {error}"));
        assert_eq!(first.submission_id, retry.submission_id);
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.command_id.clone())
                .collect::<Vec<_>>(),
            retry
                .items
                .iter()
                .map(|item| item.command_id.clone())
                .collect::<Vec<_>>(),
        );
        assert_ne!(first.items[0].command_id, first.items[1].command_id);

        let different_operation = first_handler
            .execute(
                &system,
                submission(
                    source_id,
                    vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: create(source_id),
                    }],
                    1,
                    key,
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("different operation: {error}"));
        assert_eq!(first.submission_id, different_operation.submission_id);
        assert_ne!(
            first.items[0].command_id,
            different_operation.items[0].command_id
        );
    }

    #[tokio::test]
    async fn repeated_listing_keys_keep_separate_original_indices_and_command_ids() {
        let source_id = ListingSourceId::new();
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(
                    source_id,
                    vec![
                        IndexedProductListingIngestionIntent {
                            index: 0,
                            intent: upsert(source_id, "same-source-key"),
                        },
                        IndexedProductListingIngestionIntent {
                            index: 1,
                            intent: upsert(source_id, "same-source-key"),
                        },
                    ],
                    2,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("repeated keys are independent intents: {error}"));

        assert_eq!(
            vec![0, 1],
            result
                .items
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>()
        );
        assert_ne!(result.items[0].command_id, result.items[1].command_id);
        let calls = captured_calls(&handler.publisher);
        assert_eq!(2, calls[0].len());
    }

    #[tokio::test]
    async fn mixed_publisher_outcomes_retain_each_per_item_status() {
        let source_id = ListingSourceId::new();
        let publisher = FakePublisher {
            calls: Default::default(),
            outcomes: vec![
                ProductListingIngestionOutcome::Accepted,
                ProductListingIngestionOutcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "QUEUE_REJECTED".to_owned(),
                    },
                    retryable: true,
                },
                ProductListingIngestionOutcome::Unconfirmed,
                ProductListingIngestionOutcome::NotAttempted {
                    reason: ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                },
            ],
        };
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(
                    source_id,
                    (0..4)
                        .map(|index| IndexedProductListingIngestionIntent {
                            index,
                            intent: upsert(source_id, &format!("source-{index}")),
                        })
                        .collect(),
                    4,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("mixed outcomes: {error}"));
        assert_eq!(
            ProductListingIngestionOutcome::Accepted,
            result.items[0].outcome
        );
        assert!(matches!(
            result.items[1].outcome,
            ProductListingIngestionOutcome::Rejected {
                retryable: true,
                ..
            }
        ));
        assert_eq!(
            ProductListingIngestionOutcome::Unconfirmed,
            result.items[2].outcome
        );
        assert_eq!(
            ProductListingIngestionOutcome::NotAttempted {
                reason: ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            },
            result.items[3].outcome,
        );
        assert_eq!(1, result.confirmed_accepted_count());
    }

    #[tokio::test]
    async fn duplicate_and_out_of_range_indices_fail_before_publication() {
        for items in [
            vec![
                IndexedProductListingIngestionIntent {
                    index: 0,
                    intent: upsert(ListingSourceId::new(), "a"),
                },
                IndexedProductListingIngestionIntent {
                    index: 0,
                    intent: upsert(ListingSourceId::new(), "b"),
                },
            ],
            vec![IndexedProductListingIngestionIntent {
                index: 1,
                intent: upsert(ListingSourceId::new(), "a"),
            }],
        ] {
            let publisher = FakePublisher::default();
            let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
            let source_id = items[0].intent.listing_source_id();
            assert!(matches!(
                handler
                    .execute(
                        &context(Principal::System),
                        submission(source_id, items, 1, "key"),
                    )
                    .await,
                Err(ProductListingIngestionSubmissionError::InconsistentInputIndices)
            ));
            assert!(captured_calls(&handler.publisher).is_empty());
        }
    }

    #[tokio::test]
    async fn mismatched_source_is_rejected_and_does_not_reach_publisher() {
        let batch_source = ListingSourceId::new();
        let command_source = ListingSourceId::new();
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        let result = handler
            .execute(
                &context(Principal::System),
                submission(
                    batch_source,
                    vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: upsert(command_source, "source"),
                    }],
                    1,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("source mismatch is per-item: {error}"));
        assert!(matches!(
            result.items[0].outcome,
            ProductListingIngestionOutcome::Rejected {
                reason: ProductListingIngestionRejectionReason::ListingSourceMismatch,
                ..
            }
        ));
        assert!(captured_calls(&handler.publisher).is_empty());
    }

    #[tokio::test]
    async fn raw_capture_metadata_and_evidence_are_forwarded_unchanged() {
        let source_id = ListingSourceId::new();
        let intent = raw_capture(source_id, "provider-record");
        let expected = intent.clone();
        let publisher = FakePublisher::default();
        let handler = SubmitInternalProductListingIngestionHandler::new(publisher);
        handler
            .execute(
                &context(Principal::Service("shopify".to_owned())),
                submission(
                    source_id,
                    vec![IndexedProductListingIngestionIntent { index: 0, intent }],
                    1,
                    "key",
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("raw capture submission: {error}"));
        let calls = captured_calls(&handler.publisher);
        assert_eq!(expected, calls[0][0].intent);
        assert_eq!("request-1", calls[0][0].metadata.request_id.as_str());
        assert_eq!(
            "correlation-1",
            calls[0][0].metadata.correlation_id.as_str()
        );
    }

    #[test]
    fn identity_scope_separates_actor_source_key_operation_and_index() {
        let source_id = ListingSourceId::new();
        let key = ProductListingIngestionIdempotencyKey::new("key")
            .unwrap_or_else(|error| panic!("valid key: {error}"));
        let actor = ProductListingIngestionActor::System;
        let seed = IngestionIdentitySeed::new(&actor, source_id, &key);
        let other_actor = IngestionIdentitySeed::new(
            &ProductListingIngestionActor::Service("service".into()),
            source_id,
            &key,
        );
        let other_source = IngestionIdentitySeed::new(&actor, ListingSourceId::new(), &key);
        let other_key = ProductListingIngestionIdempotencyKey::new("other-key")
            .unwrap_or_else(|error| panic!("valid key: {error}"));
        let other_key_seed = IngestionIdentitySeed::new(&actor, source_id, &other_key);

        let original = seed.command_id(ProductListingIngestionOperation::Upsert, 0);
        assert_ne!(
            original,
            other_actor.command_id(ProductListingIngestionOperation::Upsert, 0)
        );
        assert_ne!(
            original,
            other_source.command_id(ProductListingIngestionOperation::Upsert, 0)
        );
        assert_ne!(
            original,
            other_key_seed.command_id(ProductListingIngestionOperation::Upsert, 0)
        );
        assert_ne!(
            original,
            seed.command_id(ProductListingIngestionOperation::Create, 0)
        );
        assert_ne!(
            original,
            seed.command_id(ProductListingIngestionOperation::Upsert, 1)
        );

        let user_id = UserId::new();
        let user_seed = IngestionIdentitySeed::new(
            &ProductListingIngestionActor::User(user_id),
            source_id,
            &key,
        );
        let delegated_user_seed = IngestionIdentitySeed::new(
            &ProductListingIngestionActor::DelegatedUser(user_id),
            source_id,
            &key,
        );
        assert_eq!(
            user_seed.command_id(ProductListingIngestionOperation::Upsert, 0),
            delegated_user_seed.command_id(ProductListingIngestionOperation::Upsert, 0),
        );
    }
}
