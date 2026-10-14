//! Listing-owned delivery assertions. Provider lookup absence never selects a default.
use listing_source_core::{Domain, ListingIngestionMethod, ListingSourceId};
use shipping_core::{
    InvalidShippingValue,
    inputs::ListingRateInputs,
    selection::{DeliveryOverrides, ListingDeliveryConfiguration, ProfileSelection},
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeliveryIntegration {
    #[default]
    Source,
    Provider {
        method: ListingIngestionMethod,
        domain: Domain,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingDelivery {
    source_id: ListingSourceId,
    integration: DeliveryIntegration,
    selection: Option<ProfileSelection>,
    rate_inputs: ListingRateInputs,
    overrides: DeliveryOverrides,
}
impl ListingDelivery {
    pub fn new(
        source_id: ListingSourceId,
        integration: DeliveryIntegration,
        selection: Option<ProfileSelection>,
        rate_inputs: ListingRateInputs,
        overrides: DeliveryOverrides,
    ) -> Result<Self, InvalidShippingValue> {
        if let DeliveryIntegration::Provider { method, domain } = &integration
            && (!matches!(
                method,
                ListingIngestionMethod::Shopify | ListingIngestionMethod::Woocommerce
            ) || domain.as_str().len() > 253)
        {
            return Err(InvalidShippingValue::Bounds);
        }
        // Validate every source-scoped leaf even when assignment lookup is unresolved.
        ListingDeliveryConfiguration::new(
            source_id,
            selection
                .clone()
                .unwrap_or(ProfileSelection::SuppressInheritance),
            rate_inputs.clone(),
            overrides.clone(),
        )?;
        Ok(Self {
            source_id,
            integration,
            selection,
            rate_inputs,
            overrides,
        })
    }
    pub fn inherited(source_id: ListingSourceId) -> Self {
        Self {
            source_id,
            integration: DeliveryIntegration::Source,
            selection: Some(ProfileSelection::InheritSourceDefault),
            rate_inputs: ListingRateInputs::default(),
            overrides: DeliveryOverrides::default(),
        }
    }
    /// Remove current assertions without affirming a provider's default assignment.
    pub fn cleared(&self) -> Option<Self> {
        match self.integration {
            DeliveryIntegration::Source => None,
            DeliveryIntegration::Provider { .. } => Some(Self {
                source_id: self.source_id,
                integration: self.integration.clone(),
                selection: None,
                rate_inputs: ListingRateInputs::default(),
                overrides: DeliveryOverrides::default(),
            }),
        }
    }
    pub fn source_id(&self) -> ListingSourceId {
        self.source_id
    }
    pub fn integration(&self) -> &DeliveryIntegration {
        &self.integration
    }
    pub fn selection(&self) -> Option<&ProfileSelection> {
        self.selection.as_ref()
    }
    pub fn rate_inputs(&self) -> &ListingRateInputs {
        &self.rate_inputs
    }
    pub fn overrides(&self) -> &DeliveryOverrides {
        &self.overrides
    }
    /// Safe evaluator input. Missing assignment retains independent item overrides only.
    /// Callers must also report unresolved assignment when `selection()` is absent.
    pub fn configuration(&self) -> ListingDeliveryConfiguration {
        ListingDeliveryConfiguration::new(
            self.source_id,
            self.selection
                .clone()
                .unwrap_or(ProfileSelection::SuppressInheritance),
            self.rate_inputs.clone(),
            self.overrides.clone(),
        )
        .expect("validated delivery")
    }
}
