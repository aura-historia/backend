use super::admin_product_listings::AdminProductListingMutationResult;
use super::{
    product_listing_change::{PersistProductListingChangeError, persist_product_listing_change},
    update_product_listing::{
        ProductListingPatchError, UpdateProductListingCommand, apply_product_listing_update,
    },
};
use crate::{
    ports::{
        ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory,
        ProductListingStorageVersion,
    },
    use_cases::admin_product_listing_error::AdminProductListingError,
};
use application::idempotency::{self as retry, CommandIntent, CommandOutcome};
use application::{
    error::box_error,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::AuctionReferenceValidatorFactory;
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use serde_json::{Value, json};

/// Internal transaction machinery. Each inbound use case authorizes before calling it.
pub(crate) struct ProductListingManagementMutation<U, R, E, V> {
    unit_of_work: U,
    products: R,
    events: E,
    auction_references: V,
}
impl<U, R, E, V> ProductListingManagementMutation<U, R, E, V> {
    pub(crate) fn new(unit_of_work: U, products: R, events: E, auction_references: V) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            auction_references,
        }
    }
}

pub(crate) enum Action {
    Update(Box<UpdateProductListingCommand>),
    Withdraw,
    Restore,
}
impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Update(_) => "update",
            Self::Withdraw => "withdraw",
            Self::Restore => "restore",
        }
    }
}

impl<U, R, E, V> ProductListingManagementMutation<U, R, E, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    pub(crate) async fn mutate(
        &self,
        context: &OperationContext,
        id: ProductListingId,
        expected_version: ProductListingStorageVersion,
        action: Action,
        listing_source_id: Option<ListingSourceId>,
        audit_event: &'static str,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError> {
        let action_name = action.name();
        let (operation, patch) = match &action {
            Action::Update(p) => ("UPDATE_PRODUCT_LISTING", patch_value(p)),
            Action::Withdraw => ("WITHDRAW_PRODUCT_LISTING", Value::Null),
            Action::Restore => ("RESTORE_PRODUCT_LISTING", Value::Null),
        };
        // Protected and administrator aliases share the listing operation after current authority.
        let intent = CommandIntent::new(
            context,
            operation,
            id.to_string(),
            None,
            json!({"product_listing_id":id,"expected_version":expected_version.into_inner(),"patch":patch}),
        );
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            AdminProductListingError::BeginTransactionFailed(box_error(source))
        })?;
        let loaded = {
            let mut products = self.products.in_transaction(&mut tx);
            match listing_source_id {
                Some(source) => products.find_by_id_in_source(source, id).await?,
                None => products.find_by_id(id).await?,
            }
        }
        .ok_or(AdminProductListingError::NotFound)?;
        if listing_source_id.is_some_and(|source| loaded.value.listing_source_id() != source) {
            return Err(AdminProductListingError::NotFound);
        }
        if let Some(result) = retry::replay(&mut tx, intent.as_ref()).await? {
            tx.commit()
                .await
                .map_err(|e| AdminProductListingError::CommitTransactionFailed(box_error(e)))?;
            return Ok(result);
        }
        if loaded.version != expected_version {
            return Err(AdminProductListingError::ConcurrencyConflict);
        }
        let mut product = loaded.value;
        match action {
            Action::Update(patch) => {
                if product.lifecycle() == ListingLifecycle::Withdrawn {
                    return Err(AdminProductListingError::ListingWithdrawn);
                }
                apply_product_listing_update(
                    &mut tx,
                    &self.auction_references,
                    &mut product,
                    *patch,
                )
                .await
                .map_err(map_patch_error)?;
            }
            Action::Withdraw => {
                product
                    .withdraw()
                    .map_err(|source| AdminProductListingError::InvalidPatch {
                        source: box_error(source),
                    })?;
            }
            Action::Restore => {
                product
                    .restore()
                    .map_err(|source| AdminProductListingError::InvalidPatch {
                        source: box_error(source),
                    })?;
            }
        }
        let persisted = persist_product_listing_change(
            &mut tx,
            &self.products,
            &self.events,
            &mut product,
            loaded.version,
        )
        .await
        .map_err(|error| match error {
            PersistProductListingChangeError::Repository(source) => {
                AdminProductListingError::from(source)
            }
            PersistProductListingChangeError::Event(
                crate::ports::ProductListingEventAppendError::TemporarilyUnavailable { source },
            ) => AdminProductListingError::TemporarilyUnavailable { source },
            PersistProductListingChangeError::Event(source) => AdminProductListingError::Internal {
                source: box_error(source),
            },
        })?;
        let result = AdminProductListingMutationResult {
            product_listing_id: id,
            expected_version: persisted.version,
            outcome: persisted.outcome,
        };
        retry::complete(&mut tx, intent.as_ref(), &result).await?;
        tx.commit().await.map_err(|source| {
            AdminProductListingError::CommitTransactionFailed(box_error(source))
        })?;
        tracing::info!(event = audit_event, listing_source_id = %product.listing_source_id(), actor_type = context.principal.kind(), actor_id = %context.principal.label(), request_id = %context.request_id, correlation_id = %context.correlation_id, action = action_name, product_listing_id = %id, expected_version = %expected_version, result_version = %persisted.version, event_id = ?persisted.event_id, changed_fields = ?persisted.changed_fields, outcome = if persisted.outcome.changed() { "CHANGED" } else { "UNCHANGED" }, status = "success");
        Ok(AdminProductListingMutationResult {
            product_listing_id: id,
            expected_version: persisted.version,
            outcome: persisted.outcome,
        })
    }
}

fn map_patch_error(error: ProductListingPatchError) -> AdminProductListingError {
    match error {
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::NotFound,
        ) => AdminProductListingError::AuctionNotFound,
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::ListingSourceMismatch,
        ) => AdminProductListingError::AuctionSourceMismatch,
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::TemporarilyUnavailable {
                source,
            },
        ) => AdminProductListingError::TemporarilyUnavailable { source },
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::InvalidPersistedState {
                source,
            },
        ) => AdminProductListingError::InvalidPersistedState { source },
        error => AdminProductListingError::InvalidPatch {
            source: box_error(error),
        },
    }
}
impl From<ProductListingRepositoryError> for AdminProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductListingRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ProductListingRepositoryError::QueryFailed { source } => Self::Internal { source },
            error => Self::InvalidPersistedState {
                source: box_error(error),
            },
        }
    }
}

// V1 outcome contains only identity, committed storage revision and change classification.
impl CommandOutcome for AdminProductListingMutationResult {
    fn encode(&self) -> Result<Value, application::error::BoxError> {
        Ok(
            json!({"product_listing_id":self.product_listing_id,"version":self.expected_version.into_inner(),"changed":self.outcome.changed()}),
        )
    }
    fn decode(v: Value) -> Result<Self, application::error::BoxError> {
        Ok(Self {
            product_listing_id: retry::field(&v, "product_listing_id")?,
            expected_version: ProductListingStorageVersion::try_from(retry::field::<u64>(
                &v, "version",
            )?)?,
            outcome: if retry::field(&v, "changed")? {
                domain_primitives::change_outcome::ChangeOutcome::Changed
            } else {
                domain_primitives::change_outcome::ChangeOutcome::Unchanged
            },
        })
    }
}
fn patch_value(p: &super::update_product_listing::UpdateProductListingCommand) -> Value {
    use super::update_product_listing::UpdateProductListingCommand;
    let UpdateProductListingCommand {
        price,
        price_estimate_min,
        price_estimate_max,
        availability,
        url,
        images,
        auction,
    } = p;
    let money = |v: &money::Price| json!({"amount":u64::from(v.monetary_amount),"currency":v.currency.as_str()});
    json!({"price":retry::patch(price,|v|match v {product_listing_core::product_listing_price::ProductListingPrice::OnRequest=>json!({"kind":"ON_REQUEST"}),product_listing_core::product_listing_price::ProductListingPrice::Monetary(p)=>json!({"kind":"MONETARY","value":money(p)})}),
 "price_estimate_min":retry::patch(price_estimate_min,money),"price_estimate_max":retry::patch(price_estimate_max,money),
 "availability":retry::patch(availability,|v|json!(v.as_str())),"url":retry::patch(url,|v|json!(v)),
 // Image order selects the primary image and is significant.
 "images":retry::patch(images,|v|json!(v.iter().map(|i|i.url()).collect::<Vec<_>>())),
 "auction":retry::patch(auction,|v|json!({"auction_id":retry::patch(&v.auction_id,|v|json!(v)),"lot_number":retry::patch(&v.lot_number,|v|json!(v.as_str())),"catalogue_position":retry::patch(&v.catalogue_position,|v|json!(v.value())),"bidding_opens":retry::patch(&v.bidding_opens,|v|json!(v.unix_timestamp_nanos().to_string())),"scheduled_closes":retry::patch(&v.scheduled_closes,|v|json!(v.unix_timestamp_nanos().to_string())),"reported_closed_at":retry::patch(&v.reported_closed_at,|v|json!(v.unix_timestamp_nanos().to_string()))}))})
}
