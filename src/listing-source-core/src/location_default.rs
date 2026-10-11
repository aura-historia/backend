use crate::ListingSourceId;
use domain_primitives::change_outcome::ChangeOutcome;
use party_core::party_location_id::PartyLocationId;

/// Independent assertions about the same selected site. Neither asserts a seller or shipping role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLocationMeaning {
    AssociatedBusiness,
    AdvertisedInventory,
}
impl SourceLocationMeaning {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssociatedBusiness => "ASSOCIATED_BUSINESS",
            Self::AdvertisedInventory => "ADVERTISED_INVENTORY",
        }
    }
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "ASSOCIATED_BUSINESS" => Some(Self::AssociatedBusiness),
            "ADVERTISED_INVENTORY" => Some(Self::AdvertisedInventory),
            _ => None,
        }
    }
}

/// Opaque caller-supplied, source-scoped evidence. No acquisition or confidence inference.
#[derive(Clone, PartialEq, Eq)]
pub struct SourceLocationEvidence {
    reference: String,
    scope: SourceLocationMeaning,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSourceLocationAssignment {
    #[error("evidence must be nonblank, single-line and at most 2048 UTF-8 bytes")]
    InvalidEvidence,
    #[error("evidence scope does not establish the requested applicability")]
    EvidenceScopeMismatch,
}
impl SourceLocationEvidence {
    pub fn new(
        reference: String,
        scope: SourceLocationMeaning,
    ) -> Result<Self, InvalidSourceLocationAssignment> {
        if reference.trim().is_empty()
            || reference.len() > 2048
            || reference
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
        {
            return Err(InvalidSourceLocationAssignment::InvalidEvidence);
        }
        Ok(Self { reference, scope })
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn scope(&self) -> SourceLocationMeaning {
        self.scope
    }
}
impl std::fmt::Debug for SourceLocationEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceLocationEvidence(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocationAssignment {
    location_id: PartyLocationId,
    business_association: Option<SourceLocationEvidence>,
    advertised_inventory: Option<SourceLocationEvidence>,
}
impl SourceLocationAssignment {
    pub fn new(
        location_id: PartyLocationId,
        business_association: Option<SourceLocationEvidence>,
        advertised_inventory: Option<SourceLocationEvidence>,
    ) -> Result<Self, InvalidSourceLocationAssignment> {
        if business_association
            .as_ref()
            .is_some_and(|e| e.scope() != SourceLocationMeaning::AssociatedBusiness)
            || advertised_inventory
                .as_ref()
                .is_some_and(|e| e.scope() != SourceLocationMeaning::AdvertisedInventory)
        {
            return Err(InvalidSourceLocationAssignment::EvidenceScopeMismatch);
        }
        Ok(Self {
            location_id,
            business_association,
            advertised_inventory,
        })
    }
    pub fn location_id(&self) -> PartyLocationId {
        self.location_id
    }
    pub fn business_association(&self) -> Option<&SourceLocationEvidence> {
        self.business_association.as_ref()
    }
    pub fn advertised_inventory(&self) -> Option<&SourceLocationEvidence> {
        self.advertised_inventory.as_ref()
    }
    pub fn applies_to(&self, meaning: SourceLocationMeaning) -> bool {
        match meaning {
            SourceLocationMeaning::AssociatedBusiness => self.business_association.is_some(),
            SourceLocationMeaning::AdvertisedInventory => self.advertised_inventory.is_some(),
        }
    }
}

/// Source-owned configuration, separate from the Party and from listing assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceLocationDefault {
    source_id: ListingSourceId,
    assignment: Option<SourceLocationAssignment>,
}
impl ListingSourceLocationDefault {
    pub fn empty(source_id: ListingSourceId) -> Self {
        Self {
            source_id,
            assignment: None,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(
        source_id: ListingSourceId,
        assignment: Option<SourceLocationAssignment>,
    ) -> Self {
        Self {
            source_id,
            assignment,
        }
    }
    pub fn source_id(&self) -> ListingSourceId {
        self.source_id
    }
    pub fn assignment(&self) -> Option<&SourceLocationAssignment> {
        self.assignment.as_ref()
    }
    pub fn replace(&mut self, assignment: Option<SourceLocationAssignment>) -> ChangeOutcome {
        if self.assignment == assignment {
            ChangeOutcome::Unchanged
        } else {
            self.assignment = assignment;
            ChangeOutcome::Changed
        }
    }
}

/// Future G11 assertions are whole values and are resolved separately for each meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingLocationIntent {
    Inherit,
    Assert,
    Suppress,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveLocationSelection {
    ListingAssertion,
    SourceDefault,
    Absent,
}
pub fn select_effective_location(
    intent: ListingLocationIntent,
    eligible_applicable_default: bool,
) -> EffectiveLocationSelection {
    match intent {
        ListingLocationIntent::Assert => EffectiveLocationSelection::ListingAssertion,
        ListingLocationIntent::Suppress => EffectiveLocationSelection::Absent,
        ListingLocationIntent::Inherit if eligible_applicable_default => {
            EffectiveLocationSelection::SourceDefault
        }
        ListingLocationIntent::Inherit => EffectiveLocationSelection::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(scope: SourceLocationMeaning) -> SourceLocationEvidence {
        SourceLocationEvidence::new("Caller observation".to_owned(), scope).unwrap()
    }
    #[test]
    fn one_reference_has_independent_meanings_and_explicit_evidence() {
        let a = PartyLocationId::new();
        let b = PartyLocationId::new();
        let business = SourceLocationAssignment::new(
            a,
            Some(evidence(SourceLocationMeaning::AssociatedBusiness)),
            None,
        )
        .unwrap();
        assert!(business.applies_to(SourceLocationMeaning::AssociatedBusiness));
        assert!(!business.applies_to(SourceLocationMeaning::AdvertisedInventory));
        let mut source = ListingSourceLocationDefault::empty(ListingSourceId::new());
        assert!(source.assignment().is_none());
        assert!(!source.replace(None).changed());
        assert!(source.replace(Some(business.clone())).changed());
        assert!(!source.replace(Some(business)).changed());
        let inventory = SourceLocationAssignment::new(
            b,
            None,
            Some(evidence(SourceLocationMeaning::AdvertisedInventory)),
        )
        .unwrap();
        assert!(source.replace(Some(inventory)).changed());
        assert_eq!(b, source.assignment().unwrap().location_id());
        assert!(
            !source
                .assignment()
                .unwrap()
                .applies_to(SourceLocationMeaning::AssociatedBusiness)
        );
        assert!(source.replace(None).changed());
        assert!(source.assignment().is_none());
        assert!(
            SourceLocationAssignment::new(
                a,
                None,
                Some(evidence(SourceLocationMeaning::AssociatedBusiness))
            )
            .is_err()
        );
        for value in ["", "  ", "\nref", "x\u{2028}x", &"é".repeat(1025)] {
            assert!(
                SourceLocationEvidence::new(
                    value.to_owned(),
                    SourceLocationMeaning::AdvertisedInventory
                )
                .is_err()
            );
        }
        assert!(
            !format!("{:?}", evidence(SourceLocationMeaning::AdvertisedInventory))
                .contains("Caller observation")
        );
    }
    #[test]
    fn assertions_and_suppression_override_whole_defaults_per_meaning() {
        for default in [false, true] {
            assert_eq!(
                EffectiveLocationSelection::ListingAssertion,
                select_effective_location(ListingLocationIntent::Assert, default)
            );
            assert_eq!(
                EffectiveLocationSelection::Absent,
                select_effective_location(ListingLocationIntent::Suppress, default)
            );
        }
        assert_eq!(
            EffectiveLocationSelection::Absent,
            select_effective_location(ListingLocationIntent::Inherit, false)
        );
        assert_eq!(
            EffectiveLocationSelection::SourceDefault,
            select_effective_location(ListingLocationIntent::Inherit, true)
        );
        // Clearing an assertion returns to Inherit; suppression remains independently explicit.
        let business = true;
        let inventory = false;
        assert_eq!(
            EffectiveLocationSelection::SourceDefault,
            select_effective_location(ListingLocationIntent::Inherit, business)
        );
        assert_eq!(
            EffectiveLocationSelection::Absent,
            select_effective_location(ListingLocationIntent::Inherit, inventory)
        );
    }
}
