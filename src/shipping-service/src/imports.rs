//! Incremental imports use the ordinary write transactions. Acquisition is explicit;
//! request identity, producer identity and optimistic revisions have separate purposes.
use crate::{ShippingError, ports::ShippingEvidence, use_cases::*};
use application::{operation_context::OperationContext, patch_field::PatchField};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    delivery::DeliveryIntegration,
    product_listing_id::{ProductListingId, ProductListingKey},
};
use shipping_core::{
    inputs::ListingRateInputs,
    profile::{ShippingProfileContent, ShippingProfileId},
    selection::{ComponentOverride, ProfileSelection},
    terms::{CollectionPolicy, ShippingPolicy},
};
use time::OffsetDateTime;

/// A new acquisition, never the time a queued command was received.
#[derive(Debug, Clone, PartialEq)]
pub struct ShippingObservation {
    pub acquired_at: OffsetDateTime,
    pub fresh_until: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}
impl ShippingObservation {
    pub fn validate(&self, now: OffsetDateTime) -> Result<(), ShippingError> {
        if self.acquired_at > now
            || self.fresh_until < self.acquired_at
            || self.expires_at < self.fresh_until
            || self.expires_at <= now
            || self.expires_at - self.acquired_at > time::Duration::days(30)
        {
            return Err(ShippingError::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ShippingImportWrite {
    pub integration: DeliveryIntegration,
    pub sequence: u64,
    pub acquired_at: OffsetDateTime,
    pub request_key: String,
    /// Required for creation; immutable identity, independent of request_key.
    pub object_key: Option<String>,
    /// Supplying this explicitly asserts a genuinely new successful acquisition.
    pub observation: Option<ShippingObservation>,
}
pub(crate) fn validate_key(key: &str) -> Result<(), ShippingError> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ShippingError::InvalidInput);
    }
    Ok(())
}
impl ShippingImportWrite {
    pub fn validate(&self) -> Result<(), ShippingError> {
        validate_key(&self.request_key)?;
        if let Some(key) = &self.object_key {
            validate_key(key)?;
        }
        if self.sequence == 0 || self.sequence > i64::MAX as u64 {
            return Err(ShippingError::InvalidInput);
        }
        // Acquisition windows are checked after replay detection, so a committed retry
        // remains recognizable after its original evidence has expired.
        Ok(())
    }
}

pub enum ShippingImportOperation<'a> {
    CreateProfile {
        creation_key: &'a str,
        content: &'a ShippingProfileContent,
        evidence: &'a [ShippingEvidence],
    },
    ReplaceProfile(&'a ReplaceProfileCommand),
    ProfileLifecycle {
        command: &'a ProfileLifecycleCommand,
        restore: bool,
    },
    AssignDefault(&'a AssignDefaultCommand),
    ClearDefault(&'a ClearDefaultCommand),
    Listing {
        key: &'a ProductListingKey,
        expected_version: u64,
        patch: ImportListingPatch<'a>,
    },
}
pub enum ImportListingPatch<'a> {
    Unchanged,
    Clear,
    Set {
        integration: &'a DeliveryIntegration,
        selection: &'a PatchField<ProfileSelection>,
        rate_inputs: &'a PatchField<ListingRateInputs>,
        shipping: &'a PatchField<ComponentOverride<ShippingPolicy>>,
        collection: &'a PatchField<ComponentOverride<CollectionPolicy>>,
    },
}
impl ImportListingPatch<'_> {
    /// Removals carry ordering metadata without claiming a new observation.
    pub fn asserts_content(&self) -> bool {
        match self {
            Self::Unchanged | Self::Clear => false,
            Self::Set {
                selection,
                rate_inputs,
                shipping,
                collection,
                ..
            } => {
                matches!(selection, PatchField::Set(_))
                    || matches!(rate_inputs, PatchField::Set(v) if v != &ListingRateInputs::default())
                    || !matches!(
                        shipping,
                        PatchField::Unchanged | PatchField::Set(ComponentOverride::Inherit)
                    )
                    || !matches!(
                        collection,
                        PatchField::Unchanged | PatchField::Set(ComponentOverride::Inherit)
                    )
            }
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub enum ShippingImportSubject {
    Profile(ShippingProfileId),
    SourceDefault,
    Listing(ProductListingId),
}
pub struct PreparedShippingImport {
    pub replay: bool,
    pub profile_id: Option<ShippingProfileId>,
    pub integration: DeliveryIntegration,
}

#[async_trait::async_trait]
pub trait ShippingImportAccess: Send {
    /// An explicit manual assertion can take ownership even when its normalized value is equal.
    async fn manual_takeover(
        &mut self,
        source: ListingSourceId,
        operation: &ShippingImportOperation<'_>,
        subject: ShippingImportSubject,
    ) -> Result<(), ShippingError>;

    /// Source/grant authorization is already held by the owning write use case.
    async fn prepare(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
        write: &ShippingImportWrite,
        operation: &ShippingImportOperation<'_>,
    ) -> Result<PreparedShippingImport, ShippingError>;
    async fn complete(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
        write: &ShippingImportWrite,
        operation: &ShippingImportOperation<'_>,
        subject: ShippingImportSubject,
    ) -> Result<(), ShippingError>;
}
pub trait ShippingImportAccessFactory<Tx>: Send + Sync {
    fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl ShippingImportAccess + 'a;
}

/// Optional producer-scoped identity filter on the ordinary profile directory.
#[derive(Debug, Clone)]
pub struct ProfileImportIdentity {
    pub object_key: String,
    pub integration: DeliveryIntegration,
}
