use crate::ports::{
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryFactory, ProductListingStorageVersion, ProductListingWriteEffects,
    product_listing_delivery::*, stamp_product_listing_event,
};
use application::{
    error::{BoxError, box_error},
    operation_context::{CredentialCapability, OperationContext},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::change_outcome::ChangeOutcome;
use product_listing_core::{
    delivery::{DeliveryIntegration, ListingDelivery},
    listing_lifecycle::ListingLifecycle,
    product_listing_id::{ProductListingId, ProductListingKey},
};
use shipping_core::{
    inputs::ListingRateInputs,
    selection::{ComponentOverride, DeliveryOverrides, ProfileSelection},
    terms::{CollectionPolicy, ShippingPolicy},
};
use shipping_service::imports::{ImportListingPatch, ShippingImportOperation, ShippingImportWrite};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingDeliveryPatch {
    /// Every class, variant and policy reference is interpreted in this one integration scope.
    pub integration: DeliveryIntegration,
    pub selection: PatchField<ProfileSelection>,
    pub rate_inputs: PatchField<ListingRateInputs>,
    pub shipping: PatchField<ComponentOverride<ShippingPolicy>>,
    pub collection: PatchField<ComponentOverride<CollectionPolicy>>,
}
impl ProductListingDeliveryPatch {
    fn import_patch(&self) -> ImportListingPatch<'_> {
        ImportListingPatch::Set {
            integration: &self.integration,
            selection: &self.selection,
            rate_inputs: &self.rate_inputs,
            shipping: &self.shipping,
            collection: &self.collection,
        }
    }

    pub fn apply(
        &self,
        source: listing_source_core::ListingSourceId,
        current: Option<&ListingDelivery>,
    ) -> Result<ListingDelivery, ProductListingDeliveryError> {
        if current.is_some_and(|v| {
            v.integration() != &self.integration
                && (v.selection().is_some()
                    || v.rate_inputs() != &ListingRateInputs::default()
                    || v.overrides() != &DeliveryOverrides::default())
        }) {
            return Err(ProductListingDeliveryError::IntegrationMismatch);
        }
        let initial = match &self.integration {
            DeliveryIntegration::Source => ListingDelivery::inherited(source),
            DeliveryIntegration::Provider { .. } => ListingDelivery::new(
                source,
                self.integration.clone(),
                None,
                ListingRateInputs::default(),
                DeliveryOverrides::default(),
            )
            .map_err(|source| ProductListingDeliveryError::InvalidInput {
                source: box_error(source),
            })?,
        };
        let current = current
            .filter(|v| v.integration() == &self.integration)
            .unwrap_or(&initial);
        let selection = match &self.selection {
            PatchField::Unchanged => current.selection().cloned(),
            PatchField::Clear => None,
            PatchField::Set(v) => Some(v.clone()),
        };
        let inputs = match &self.rate_inputs {
            PatchField::Unchanged => current.rate_inputs().clone(),
            PatchField::Clear => ListingRateInputs::default(),
            PatchField::Set(v) => v.clone(),
        };
        let component = |patch: &PatchField<ComponentOverride<ShippingPolicy>>| match patch {
            PatchField::Unchanged => current.overrides().shipping.clone(),
            PatchField::Clear => ComponentOverride::Clear,
            PatchField::Set(v) => v.clone(),
        };
        let collection = match &self.collection {
            PatchField::Unchanged => current.overrides().collection.clone(),
            PatchField::Clear => ComponentOverride::Clear,
            PatchField::Set(v) => v.clone(),
        };
        ListingDelivery::new(
            source,
            self.integration.clone(),
            selection,
            inputs,
            DeliveryOverrides {
                shipping: component(&self.shipping),
                collection,
            },
        )
        .map_err(|source| ProductListingDeliveryError::InvalidInput {
            source: box_error(source),
        })
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeProductListingDeliveryCommand {
    pub import: Option<ShippingImportWrite>,
    pub product_key: ProductListingKey,
    pub expected_version: ProductListingStorageVersion,
    pub delivery: PatchField<ProductListingDeliveryPatch>,
}
impl ChangeProductListingDeliveryCommand {
    pub fn import_operation(&self) -> ShippingImportOperation<'_> {
        ShippingImportOperation::Listing {
            key: &self.product_key,
            expected_version: self.expected_version.into_inner(),
            patch: match &self.delivery {
                PatchField::Unchanged => ImportListingPatch::Unchanged,
                PatchField::Clear => ImportListingPatch::Clear,
                PatchField::Set(p) => p.import_patch(),
            },
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeProductListingDeliveryResult {
    pub product_listing_id: ProductListingId,
    pub version: ProductListingStorageVersion,
    pub outcome: ChangeOutcome,
}
#[derive(Debug, thiserror::Error)]
pub enum ProductListingDeliveryError {
    #[error("authentication required")]
    AuthenticationRequired,
    #[error("source access denied")]
    Forbidden,
    #[error("listing source not found")]
    SourceNotFound,
    #[error("product listing not found")]
    NotFound,
    #[error("stale listing revision")]
    StaleRevision,
    #[error("listing is withdrawn")]
    ListingWithdrawn,
    #[error("delivery integration mismatch")]
    IntegrationMismatch,
    #[error("delivery reference is missing or ineligible")]
    ReferenceIneligible,
    #[error("delivery input invalid")]
    InvalidInput {
        #[source]
        source: BoxError,
    },
    #[error("shipping import rejected")]
    Import(
        #[from]
        #[source]
        shipping_service::ShippingError,
    ),
    #[error("delivery capability unavailable")]
    Unavailable,
    #[error("delivery operation failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("delivery transaction failed")]
    Transaction(#[source] BoxError),
}
impl ProductListingDeliveryError {
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "DELIVERY_AUTHENTICATION_REQUIRED",
            Self::Forbidden => "DELIVERY_FORBIDDEN",
            Self::SourceNotFound => "DELIVERY_SOURCE_NOT_FOUND",
            Self::NotFound => "DELIVERY_NOT_FOUND",
            Self::StaleRevision => "DELIVERY_STALE_REVISION",
            Self::ListingWithdrawn => "DELIVERY_LISTING_WITHDRAWN",
            Self::IntegrationMismatch => "DELIVERY_INTEGRATION_MISMATCH",
            Self::ReferenceIneligible => "DELIVERY_REFERENCE_INELIGIBLE",
            Self::InvalidInput { .. } => "DELIVERY_INVALID_INPUT",
            Self::Import(_) => "DELIVERY_IMPORT_REJECTED",
            Self::Unavailable => "DELIVERY_UNAVAILABLE",
            Self::OperationFailed { .. } => "DELIVERY_OPERATION_FAILED",
            Self::Transaction(_) => "DELIVERY_TRANSACTION_FAILED",
        }
    }
}
pub(crate) fn capability(
    context: &OperationContext,
    write: bool,
) -> Result<(), ProductListingDeliveryError> {
    context
        .require_credential_capability(if write {
            CredentialCapability::ProductListingsWrite
        } else {
            CredentialCapability::ProductListingsRead
        })
        .map(|_| ())
        .map_err(|_| {
            if matches!(
                context.principal,
                application::operation_context::Principal::Anonymous
            ) {
                ProductListingDeliveryError::AuthenticationRequired
            } else {
                ProductListingDeliveryError::Forbidden
            }
        })
}
fn failure(source: impl std::error::Error + Send + Sync + 'static) -> ProductListingDeliveryError {
    ProductListingDeliveryError::OperationFailed {
        source: box_error(source),
    }
}
#[async_trait::async_trait]
pub trait ChangeProductListingDeliveryUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeProductListingDeliveryCommand,
    ) -> Result<ChangeProductListingDeliveryResult, ProductListingDeliveryError>;
}
pub struct ChangeProductListingDeliveryHandler<U, R, E, V> {
    uow: U,
    products: R,
    events: E,
    references: V,
}
impl<U, R, E, V> ChangeProductListingDeliveryHandler<U, R, E, V> {
    pub fn new(uow: U, products: R, events: E, references: V) -> Self {
        Self {
            uow,
            products,
            events,
            references,
        }
    }
}
impl<U, R, E, V> ChangeProductListingDeliveryHandler<U, R, E, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    V: ProductListingDeliveryReferencesFactory<U::Tx>,
{
    pub(crate) async fn apply_in_transaction(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        command: &ChangeProductListingDeliveryCommand,
    ) -> Result<ChangeProductListingDeliveryResult, ProductListingDeliveryError> {
        capability(context, true)?;
        self.references
            .in_transaction(tx)
            .authorize(context, command.product_key.listing_source_id)
            .await?;
        let replay = if command.import.is_some() {
            self.references
                .in_transaction(tx)
                .prepare_import(context, command)
                .await?
        } else {
            false
        };
        self.references
            .in_transaction(tx)
            .lock_target(&command.product_key)
            .await?;
        let stored = self
            .products
            .in_transaction(tx)
            .find_by_key(&command.product_key)
            .await
            .map_err(failure)?
            .ok_or(ProductListingDeliveryError::NotFound)?;
        if replay {
            return Ok(ChangeProductListingDeliveryResult {
                product_listing_id: stored.value.id(),
                version: stored.version,
                outcome: ChangeOutcome::Unchanged,
            });
        }
        if stored.version != command.expected_version {
            return Err(ProductListingDeliveryError::StaleRevision);
        }
        if stored.value.lifecycle() == ListingLifecycle::Withdrawn {
            return Err(ProductListingDeliveryError::ListingWithdrawn);
        }
        let source = command.product_key.listing_source_id;
        let delivery = match &command.delivery {
            PatchField::Unchanged => stored.value.delivery().cloned(),
            PatchField::Clear => stored.value.delivery().and_then(ListingDelivery::cleared),
            PatchField::Set(p) => Some(p.apply(source, stored.value.delivery())?),
        };
        if !matches!(command.delivery, PatchField::Clear) {
            let inherited = ListingDelivery::inherited(source);
            self.references
                .in_transaction(tx)
                .validate(delivery.as_ref().unwrap_or(&inherited))
                .await?;
        }
        if let Some(write) = &command.import {
            let asserts_content = matches!(&command.delivery, PatchField::Set(p) if p.import_patch().asserts_content());
            if (asserts_content
                && delivery != stored.value.delivery().cloned()
                && write.observation.is_none())
                || (!asserts_content && write.observation.is_some())
            {
                return Err(ProductListingDeliveryError::Import(
                    shipping_service::ShippingError::InvalidInput,
                ));
            }
        }
        let mut listing = stored.value;
        let outcome = listing.replace_delivery(delivery).map_err(failure)?;
        let version = if let Some(payload) = listing.take_pending_event_payload() {
            let event =
                stamp_product_listing_event(listing.id(), time::OffsetDateTime::now_utc(), payload);
            let effects = ProductListingWriteEffects::from(&event.payload);
            let updated = self
                .products
                .in_transaction(tx)
                .update(&listing, stored.version, event.event_id, effects)
                .await
                .map_err(|e| match e {
                    crate::ports::ProductListingRepositoryError::ConcurrencyConflict => {
                        ProductListingDeliveryError::StaleRevision
                    }
                    e => failure(e),
                })?;
            self.events
                .in_transaction(tx)
                .append(&event)
                .await
                .map_err(failure)?;
            updated.version
        } else {
            stored.version
        };
        if command.import.is_some() {
            self.references
                .in_transaction(tx)
                .complete_import(context, command, listing.id())
                .await?;
        } else {
            self.references
                .in_transaction(tx)
                .manual_takeover(command, listing.id())
                .await?;
        }
        Ok(ChangeProductListingDeliveryResult {
            product_listing_id: listing.id(),
            version,
            outcome,
        })
    }
}
#[async_trait::async_trait]
impl<U, R, E, V> ChangeProductListingDeliveryUseCase
    for ChangeProductListingDeliveryHandler<U, R, E, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    V: ProductListingDeliveryReferencesFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeProductListingDeliveryCommand,
    ) -> Result<ChangeProductListingDeliveryResult, ProductListingDeliveryError> {
        capability(context, true)?;
        let mut tx = self
            .uow
            .begin()
            .await
            .map_err(|e| ProductListingDeliveryError::Transaction(box_error(e)))?;
        let result = self
            .apply_in_transaction(&mut tx, context, &command)
            .await?;
        tx.commit()
            .await
            .map_err(|e| ProductListingDeliveryError::Transaction(box_error(e)))?;
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use shipping_core::{
        inputs::{ClassMembership, FactCompleteness, SourceShippingClass, SourceVariant},
        terms::{CollectionDependency, DeliveryTerms},
    };
    #[test]
    fn coherent_component_removal_and_inheritance_have_distinct_outcomes() {
        let source = listing_source_core::ListingSourceId::new();
        let base = ListingDelivery::new(
            source,
            DeliveryIntegration::Source,
            Some(ProfileSelection::inline(DeliveryTerms::new(
                Some(ShippingPolicy::Prohibited),
                Some(CollectionPolicy::new(
                    shipping_core::inputs::Applicability::default(),
                    CollectionDependency::Independent,
                    Some(shipping_core::terms::ShippingEligibility::Offered),
                    None,
                    None,
                )),
            ))),
            ListingRateInputs::default(),
            DeliveryOverrides::default(),
        )
        .unwrap();
        let cleared = ProductListingDeliveryPatch {
            shipping: PatchField::Clear,
            ..Default::default()
        }
        .apply(source, Some(&base))
        .unwrap();
        assert_eq!(&ComponentOverride::Clear, &cleared.overrides().shipping);
        let removed = ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Inherit),
            ..Default::default()
        }
        .apply(source, Some(&cleared))
        .unwrap();
        assert_eq!(&ComponentOverride::Inherit, &removed.overrides().shipping);
        let suppressed = ProductListingDeliveryPatch {
            selection: PatchField::Set(ProfileSelection::SuppressInheritance),
            ..Default::default()
        }
        .apply(source, Some(&base))
        .unwrap();
        assert!(matches!(
            suppressed.selection(),
            Some(ProfileSelection::SuppressInheritance)
        ));
        let retracted = ProductListingDeliveryPatch {
            selection: PatchField::Clear,
            ..Default::default()
        }
        .apply(source, Some(&base))
        .unwrap();
        assert!(retracted.selection().is_none());
        assert!(matches!(
            retracted.configuration().selection(),
            ProfileSelection::SuppressInheritance
        ));
        let resumed = ProductListingDeliveryPatch {
            selection: PatchField::Set(ProfileSelection::InheritSourceDefault),
            ..Default::default()
        }
        .apply(source, Some(&suppressed))
        .unwrap();
        assert!(matches!(
            resumed.selection(),
            Some(ProfileSelection::InheritSourceDefault)
        ));
        let partial = CollectionPolicy::new(
            shipping_core::inputs::Applicability::default(),
            CollectionDependency::Independent,
            None,
            None,
            None,
        );
        let replaced = ProductListingDeliveryPatch {
            collection: PatchField::Set(ComponentOverride::Replace(partial.clone())),
            ..Default::default()
        }
        .apply(source, Some(&base))
        .unwrap();
        assert_eq!(
            &ComponentOverride::Replace(partial),
            &replaced.overrides().collection
        );
    }
    #[test]
    fn provider_absence_and_foreign_rate_input_identity_fail_closed() {
        let source = listing_source_core::ListingSourceId::new();
        let provider = DeliveryIntegration::Provider {
            method: listing_source_core::ListingIngestionMethod::Shopify,
        };
        let pending = ProductListingDeliveryPatch {
            integration: provider.clone(),
            ..Default::default()
        }
        .apply(source, None)
        .unwrap();
        assert!(pending.selection().is_none());
        assert!(matches!(
            pending.configuration().selection(),
            ProfileSelection::SuppressInheritance
        ));
        assert!(
            ProductListingDeliveryPatch::default()
                .apply(source, Some(&pending))
                .is_ok()
        );
        let asserted = ProductListingDeliveryPatch {
            integration: provider.clone(),
            selection: PatchField::Set(ProfileSelection::SuppressInheritance),
            ..Default::default()
        }
        .apply(source, Some(&pending))
        .unwrap();
        assert!(
            ProductListingDeliveryPatch::default()
                .apply(source, Some(&asserted))
                .is_err()
        );
        let foreign = listing_source_core::ListingSourceId::new();
        for inputs in [
            ListingRateInputs::new(
                Some(
                    ClassMembership::new(
                        vec![SourceShippingClass::new(foreign, "cabinet".into()).unwrap()],
                        FactCompleteness::Complete,
                    )
                    .unwrap(),
                ),
                None,
            )
            .unwrap(),
            ListingRateInputs::new(
                None,
                Some(vec![
                    SourceVariant::new(foreign, "gid://synthetic/Variant/2".into()).unwrap(),
                ]),
            )
            .unwrap(),
        ] {
            assert!(
                ProductListingDeliveryPatch {
                    rate_inputs: PatchField::Set(inputs),
                    ..Default::default()
                }
                .apply(source, None)
                .is_err()
            );
        }
    }
}
