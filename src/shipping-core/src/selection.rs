use crate::{
    InvalidShippingValue,
    inputs::ListingRateInputs,
    profile::{ShippingProfile, ShippingProfileId, ShippingProfileLifecycle},
    terms::{CollectionPolicy, DeliveryTerms, ShippingPolicy},
};
use domain_primitives::change_outcome::ChangeOutcome;
use listing_source_core::ListingSourceId;

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileSelection {
    InheritSourceDefault,
    Explicit(ShippingProfileId),
    Inline(Box<DeliveryTerms>),
    SuppressInheritance,
}
impl ProfileSelection {
    pub fn inline(terms: DeliveryTerms) -> Self {
        Self::Inline(Box::new(terms))
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InheritSourceDefault => "INHERIT_SOURCE_DEFAULT",
            Self::Explicit(_) => "EXPLICIT",
            Self::Inline(_) => "INLINE",
            Self::SuppressInheritance => "SUPPRESS_INHERITANCE",
        }
    }
}

/// Inherit removes a previous override. Clear removes the assertion from effective terms.
/// Replace swaps one coherent policy, including its geography, rates and applicability.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ComponentOverride<T> {
    #[default]
    Inherit,
    Replace(T),
    Clear,
}
impl<T> ComponentOverride<T> {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inherit => "INHERIT",
            Self::Replace(_) => "REPLACE",
            Self::Clear => "CLEAR",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeliveryOverrides {
    pub shipping: ComponentOverride<ShippingPolicy>,
    pub collection: ComponentOverride<CollectionPolicy>,
}
impl DeliveryOverrides {
    fn validate_source(&self, source_id: ListingSourceId) -> Result<(), InvalidShippingValue> {
        if let ComponentOverride::Replace(shipping) = &self.shipping {
            shipping.validate_source(source_id)?;
        }
        if let ComponentOverride::Replace(collection) = &self.collection {
            collection.applicability().validate_source(source_id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingDeliveryConfiguration {
    source_id: ListingSourceId,
    selection: ProfileSelection,
    rate_inputs: ListingRateInputs,
    overrides: DeliveryOverrides,
}
impl ListingDeliveryConfiguration {
    pub fn new(
        source_id: ListingSourceId,
        selection: ProfileSelection,
        rate_inputs: ListingRateInputs,
        overrides: DeliveryOverrides,
    ) -> Result<Self, InvalidShippingValue> {
        if let ProfileSelection::Inline(terms) = &selection {
            terms.validate_source(source_id)?;
        }
        rate_inputs.validate_source(source_id)?;
        overrides.validate_source(source_id)?;
        Ok(Self {
            source_id,
            selection,
            rate_inputs,
            overrides,
        })
    }
    pub fn source_id(&self) -> ListingSourceId {
        self.source_id
    }
    pub fn selection(&self) -> &ProfileSelection {
        &self.selection
    }
    pub fn rate_inputs(&self) -> &ListingRateInputs {
        &self.rate_inputs
    }
    pub fn overrides(&self) -> &DeliveryOverrides {
        &self.overrides
    }
    pub fn select(
        &mut self,
        selection: ProfileSelection,
    ) -> Result<ChangeOutcome, InvalidShippingValue> {
        if let ProfileSelection::Inline(terms) = &selection {
            terms.validate_source(self.source_id)?;
        }
        let outcome = ChangeOutcome::from(self.selection != selection);
        self.selection = selection;
        Ok(outcome)
    }
    pub fn replace_rate_inputs(
        &mut self,
        inputs: ListingRateInputs,
    ) -> Result<ChangeOutcome, InvalidShippingValue> {
        inputs.validate_source(self.source_id)?;
        let outcome = ChangeOutcome::from(self.rate_inputs != inputs);
        self.rate_inputs = inputs;
        Ok(outcome)
    }
    pub fn replace_overrides(
        &mut self,
        overrides: DeliveryOverrides,
    ) -> Result<ChangeOutcome, InvalidShippingValue> {
        overrides.validate_source(self.source_id)?;
        let outcome = ChangeOutcome::from(self.overrides != overrides);
        self.overrides = overrides;
        Ok(outcome)
    }
}

/// Shipping owns this configuration; no implicit Party or same-content fallback exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDefaultProfile {
    source_id: ListingSourceId,
    profile_id: Option<ShippingProfileId>,
}
impl SourceDefaultProfile {
    pub fn empty(source_id: ListingSourceId) -> Self {
        Self {
            source_id,
            profile_id: None,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(source_id: ListingSourceId, profile_id: Option<ShippingProfileId>) -> Self {
        Self {
            source_id,
            profile_id,
        }
    }
    pub fn source_id(&self) -> ListingSourceId {
        self.source_id
    }
    pub fn profile_id(&self) -> Option<ShippingProfileId> {
        self.profile_id
    }
    pub fn select(
        &mut self,
        profile: Option<&ShippingProfile>,
    ) -> Result<ChangeOutcome, InvalidShippingValue> {
        if profile.is_some_and(|p| p.source_id() != self.source_id) {
            return Err(InvalidShippingValue::SourceMismatch);
        }
        if profile.is_some_and(|p| p.lifecycle() != ShippingProfileLifecycle::Active) {
            return Err(InvalidShippingValue::RetiredProfile);
        }
        let profile_id = profile.map(ShippingProfile::id);
        let outcome = ChangeOutcome::from(self.profile_id != profile_id);
        self.profile_id = profile_id;
        Ok(outcome)
    }
}
