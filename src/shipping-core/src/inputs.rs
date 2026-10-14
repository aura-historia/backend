use crate::{InvalidShippingValue, pricing::QualifyingScope, validate_text};
use geo::GeographicDescription;
use listing_source_core::ListingSourceId;
use money::Price;
use std::num::NonZeroU64;

/// Source-owned classification and variant codes are opaque and case-sensitive.
/// They are not Aura object identities, prices or override instructions.
macro_rules! source_code {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name {
            source_id: ListingSourceId,
            code: String,
        }
        impl $name {
            pub fn new(
                source_id: ListingSourceId,
                code: String,
            ) -> Result<Self, InvalidShippingValue> {
                validate_text(&code, 128)?;
                Ok(Self { source_id, code })
            }
            pub fn source_id(&self) -> ListingSourceId {
                self.source_id
            }
            pub fn code(&self) -> &str {
                &self.code
            }
        }
    };
}
source_code!(SourceShippingClass);
source_code!(SourceVariant);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactCompleteness {
    Partial,
    Complete,
}
codes!(FactCompleteness, Partial => "PARTIAL", Complete => "COMPLETE");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassMembership {
    classes: Vec<SourceShippingClass>,
    completeness: FactCompleteness,
}
impl ClassMembership {
    pub fn new(
        classes: Vec<SourceShippingClass>,
        completeness: FactCompleteness,
    ) -> Result<Self, InvalidShippingValue> {
        validate_codes(&classes)?;
        validate_same_source(classes.iter().map(SourceShippingClass::source_id))?;
        Ok(Self {
            classes,
            completeness,
        })
    }
    pub fn classes(&self) -> &[SourceShippingClass] {
        &self.classes
    }
    pub fn completeness(&self) -> FactCompleteness {
        self.completeness
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingRateInputs {
    classes: Option<ClassMembership>,
    variants: Option<Vec<SourceVariant>>,
}
impl ListingRateInputs {
    pub fn new(
        classes: Option<ClassMembership>,
        variants: Option<Vec<SourceVariant>>,
    ) -> Result<Self, InvalidShippingValue> {
        if let Some(variants) = &variants {
            validate_variants(variants)?;
        }
        validate_same_source(
            classes
                .iter()
                .flat_map(|c| c.classes())
                .map(SourceShippingClass::source_id)
                .chain(variants.iter().flatten().map(SourceVariant::source_id)),
        )?;
        Ok(Self { classes, variants })
    }
    pub fn classes(&self) -> Option<&ClassMembership> {
        self.classes.as_ref()
    }
    pub fn variants(&self) -> Option<&[SourceVariant]> {
        self.variants.as_deref()
    }
    pub(crate) fn source_id(&self) -> Option<ListingSourceId> {
        self.classes()
            .and_then(|membership| membership.classes().first())
            .map(SourceShippingClass::source_id)
            .or_else(|| self.variants()?.first().map(SourceVariant::source_id))
    }
    pub(crate) fn validate_source(
        &self,
        source_id: ListingSourceId,
    ) -> Result<(), InvalidShippingValue> {
        if self
            .classes
            .as_ref()
            .is_some_and(|v| v.classes.iter().any(|c| c.source_id != source_id))
            || self
                .variants
                .as_ref()
                .is_some_and(|v| v.iter().any(|c| c.source_id != source_id))
        {
            return Err(InvalidShippingValue::SourceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantityRange {
    minimum: NonZeroU64,
    maximum: NonZeroU64,
}
impl QuantityRange {
    pub fn new(minimum: NonZeroU64, maximum: NonZeroU64) -> Result<Self, InvalidShippingValue> {
        if minimum > maximum {
            return Err(InvalidShippingValue::Bounds);
        }
        Ok(Self { minimum, maximum })
    }
    pub fn minimum(self) -> NonZeroU64 {
        self.minimum
    }
    pub fn maximum(self) -> NonZeroU64 {
        self.maximum
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmountThreshold {
    pub minimum: Price,
    pub scope: QualifyingScope,
}

/// Bounded conjunction of explicit facts. No user-defined expressions or recursive rules.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applicability {
    class: Option<SourceShippingClass>,
    variants: Option<Vec<SourceVariant>>,
    quantity: Option<QuantityRange>,
    threshold: Option<AmountThreshold>,
}
impl Applicability {
    pub fn new(
        class: Option<SourceShippingClass>,
        variants: Option<Vec<SourceVariant>>,
        quantity: Option<QuantityRange>,
        threshold: Option<AmountThreshold>,
    ) -> Result<Self, InvalidShippingValue> {
        if let Some(variants) = &variants {
            validate_variants(variants)?;
        }
        validate_same_source(
            class
                .iter()
                .map(SourceShippingClass::source_id)
                .chain(variants.iter().flatten().map(SourceVariant::source_id)),
        )?;
        Ok(Self {
            class,
            variants,
            quantity,
            threshold,
        })
    }
    pub fn class(&self) -> Option<&SourceShippingClass> {
        self.class.as_ref()
    }
    pub fn variants(&self) -> Option<&[SourceVariant]> {
        self.variants.as_deref()
    }
    pub fn quantity(&self) -> Option<QuantityRange> {
        self.quantity
    }
    pub fn threshold(&self) -> Option<AmountThreshold> {
        self.threshold
    }
    pub(crate) fn source_id(&self) -> Option<ListingSourceId> {
        self.class()
            .map(SourceShippingClass::source_id)
            .or_else(|| self.variants()?.first().map(SourceVariant::source_id))
    }
    pub(crate) fn validate_source(
        &self,
        source_id: ListingSourceId,
    ) -> Result<(), InvalidShippingValue> {
        if self
            .class
            .as_ref()
            .is_some_and(|c| c.source_id != source_id)
            || self
                .variants
                .as_ref()
                .is_some_and(|v| v.iter().any(|c| c.source_id != source_id))
        {
            return Err(InvalidShippingValue::SourceMismatch);
        }
        Ok(())
    }
}

pub(crate) fn validate_codes<T: PartialEq>(values: &[T]) -> Result<(), InvalidShippingValue> {
    if values.len() > 64
        || values
            .iter()
            .enumerate()
            .any(|(i, v)| values[..i].contains(v))
    {
        return Err(InvalidShippingValue::RuleCollection);
    }
    Ok(())
}

fn validate_variants(variants: &[SourceVariant]) -> Result<(), InvalidShippingValue> {
    validate_codes(variants)?;
    if variants.is_empty() {
        return Err(InvalidShippingValue::RuleCollection);
    }
    validate_same_source(variants.iter().map(SourceVariant::source_id))
}
fn validate_same_source(
    sources: impl IntoIterator<Item = ListingSourceId>,
) -> Result<(), InvalidShippingValue> {
    let mut sources = sources.into_iter();
    if let Some(first) = sources.next()
        && sources.any(|source| source != first)
    {
        return Err(InvalidShippingValue::SourceMismatch);
    }
    Ok(())
}

/// Supplied by a caller for assessment only. Counts have separate charge bases.
/// Amounts are original-currency qualifying values, never silently converted.
#[derive(Clone, Default)]
pub struct AssessmentInput {
    pub destination: Option<GeographicDescription>,
    pub quantity: Option<NonZeroU64>,
    pub lots: Option<NonZeroU64>,
    pub shipments: Option<NonZeroU64>,
    pub seller_orders: Option<NonZeroU64>,
    pub selected_variant: Option<SourceVariant>,
    pub item_value: Option<Price>,
    pub lot_value: Option<Price>,
    pub shipment_value: Option<Price>,
    pub seller_order_value: Option<Price>,
}
impl AssessmentInput {
    pub fn qualifying_amount(&self, scope: QualifyingScope) -> Option<Price> {
        match scope {
            QualifyingScope::Item => self.item_value,
            QualifyingScope::Lot => self.lot_value,
            QualifyingScope::Shipment => self.shipment_value,
            QualifyingScope::SellerOrder => self.seller_order_value,
        }
    }
}
impl std::fmt::Debug for AssessmentInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AssessmentInput(<redacted>)")
    }
}
