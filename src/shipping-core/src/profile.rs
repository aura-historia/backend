use crate::{InvalidShippingValue, terms::DeliveryTerms, validate_text};
use domain_primitives::change_outcome::ChangeOutcome;
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use std::num::NonZeroU64;

domain_primitives::object_id_newtype!(ShippingProfileId, "shp");

/// Positive semantic revision. Advancing it cannot wrap to an invalid persisted value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShippingProfileRevision(NonZeroU64);
impl ShippingProfileRevision {
    pub const INITIAL: Self = Self(NonZeroU64::MIN);
    pub fn into_inner(self) -> u64 {
        self.0.get()
    }
    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}
impl Default for ShippingProfileRevision {
    fn default() -> Self {
        Self::INITIAL
    }
}
impl TryFrom<u64> for ShippingProfileRevision {
    type Error = domain_primitives::version::InvalidVersionError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value).map(Self).ok_or(Self::Error::Zero)
    }
}
impl std::fmt::Display for ShippingProfileRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingProfileName(String);
impl ShippingProfileName {
    pub fn new(value: String) -> Result<Self, InvalidShippingValue> {
        let value = value.trim();
        validate_text(value, 255)?;
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShippingProfileLifecycle {
    Active,
    Retired,
}
codes!(ShippingProfileLifecycle, Active => "ACTIVE", Retired => "RETIRED");

#[derive(Debug, Clone, PartialEq)]
pub struct ShippingProfileContent {
    pub name: ShippingProfileName,
    /// Attribution confers no seller, carrier, arranger or quote-issuer assertion.
    pub attributed_party: Option<PartyId>,
    pub terms: DeliveryTerms,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShippingProfile {
    id: ShippingProfileId,
    source_id: ListingSourceId,
    revision: ShippingProfileRevision,
    lifecycle: ShippingProfileLifecycle,
    content: ShippingProfileContent,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShippingProfileError {
    #[error("invalid shipping profile")]
    Invalid(
        #[from]
        #[source]
        InvalidShippingValue,
    ),
    #[error("retired shipping profiles require explicit restoration")]
    Retired,
    #[error("shipping profile revision exhausted")]
    RevisionExhausted,
}
impl ShippingProfile {
    pub fn create(
        id: ShippingProfileId,
        source_id: ListingSourceId,
        content: ShippingProfileContent,
    ) -> Result<Self, ShippingProfileError> {
        Self::rehydrate(
            id,
            source_id,
            ShippingProfileRevision::INITIAL,
            ShippingProfileLifecycle::Active,
            content,
        )
    }
    #[doc(hidden)]
    pub fn rehydrate(
        id: ShippingProfileId,
        source_id: ListingSourceId,
        revision: ShippingProfileRevision,
        lifecycle: ShippingProfileLifecycle,
        content: ShippingProfileContent,
    ) -> Result<Self, ShippingProfileError> {
        content.terms.validate_source(source_id)?;
        Ok(Self {
            id,
            source_id,
            revision,
            lifecycle,
            content,
        })
    }
    pub fn replace_content(
        &mut self,
        content: ShippingProfileContent,
    ) -> Result<ChangeOutcome, ShippingProfileError> {
        content.terms.validate_source(self.source_id)?;
        if self.content == content {
            return Ok(ChangeOutcome::Unchanged);
        }
        if self.lifecycle == ShippingProfileLifecycle::Retired {
            return Err(ShippingProfileError::Retired);
        }
        self.advance_revision()?;
        self.content = content;
        Ok(ChangeOutcome::Changed)
    }
    pub fn retire(&mut self) -> Result<ChangeOutcome, ShippingProfileError> {
        self.set_lifecycle(ShippingProfileLifecycle::Retired)
    }
    pub fn restore(&mut self) -> Result<ChangeOutcome, ShippingProfileError> {
        self.set_lifecycle(ShippingProfileLifecycle::Active)
    }
    fn set_lifecycle(
        &mut self,
        lifecycle: ShippingProfileLifecycle,
    ) -> Result<ChangeOutcome, ShippingProfileError> {
        if self.lifecycle == lifecycle {
            return Ok(ChangeOutcome::Unchanged);
        }
        self.advance_revision()?;
        self.lifecycle = lifecycle;
        Ok(ChangeOutcome::Changed)
    }
    fn advance_revision(&mut self) -> Result<(), ShippingProfileError> {
        self.revision = self
            .revision
            .checked_next()
            .ok_or(ShippingProfileError::RevisionExhausted)?;
        Ok(())
    }
    pub fn id(&self) -> ShippingProfileId {
        self.id
    }
    pub fn source_id(&self) -> ListingSourceId {
        self.source_id
    }
    pub fn revision(&self) -> ShippingProfileRevision {
        self.revision
    }
    pub fn lifecycle(&self) -> ShippingProfileLifecycle {
        self.lifecycle
    }
    pub fn content(&self) -> &ShippingProfileContent {
        &self.content
    }
}
