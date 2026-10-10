use crate::{party_id::PartyId, party_location_id::PartyLocationId};
use domain_primitives::change_outcome::ChangeOutcome;
use geo::{GeographicDescription, SpatialPosition};
use std::collections::BTreeSet;

/// A label identifies a site within one Party; it never determines site identity.
#[derive(Clone, PartialEq, Eq)]
pub struct PartyLocationLabel(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("location label must be nonblank, single-line and at most 255 UTF-8 bytes")]
pub struct InvalidPartyLocationLabel;

impl PartyLocationLabel {
    pub fn new(value: String) -> Result<Self, InvalidPartyLocationLabel> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 255
            || value
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
        {
            return Err(InvalidPartyLocationLabel);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for PartyLocationLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PartyLocationLabel(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PartyLocationRole {
    BusinessPremises,
    Warehouse,
    RegisteredAddress,
    Correspondence,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartyLocationDisclosure {
    #[default]
    Private,
    CoarsePublic,
    ExactPublic,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyLocationLifecycle {
    Active,
    Retired,
}

macro_rules! codes {
    ($ty:ident, $($variant:ident => $code:literal),+ $(,)?) => {
        impl $ty {
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $code),+ } }
            pub fn from_code(value: &str) -> Option<Self> { match value { $($code => Some(Self::$variant)),+, _ => None } }
        }
    };
}
codes!(PartyLocationRole, BusinessPremises => "BUSINESS_PREMISES", Warehouse => "WAREHOUSE", RegisteredAddress => "REGISTERED_ADDRESS", Correspondence => "CORRESPONDENCE");
codes!(PartyLocationDisclosure, Private => "PRIVATE", CoarsePublic => "COARSE_PUBLIC", ExactPublic => "EXACT_PUBLIC");
codes!(PartyLocationLifecycle, Active => "ACTIVE", Retired => "RETIRED");

/// Role vocabulary bounds the set. Roles assert site use, never stock or pickup facts.
#[derive(Clone, PartialEq)]
pub struct PartyLocationContent {
    pub label: PartyLocationLabel,
    pub roles: BTreeSet<PartyLocationRole>,
    pub geography: Option<GeographicDescription>,
    /// Caller-asserted coordinates, separate from future derived resolution.
    pub position: Option<SpatialPosition>,
    pub disclosure: PartyLocationDisclosure,
}

impl std::fmt::Debug for PartyLocationContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PartyLocationContent(<redacted>)")
    }
}

domain_primitives::version_newtype!(PartyLocationInputRevision, no_serde);

#[derive(Debug, Clone, PartialEq)]
pub struct PartyLocation {
    id: PartyLocationId,
    party_id: PartyId,
    content: PartyLocationContent,
    lifecycle: PartyLocationLifecycle,
    input_revision: PartyLocationInputRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PartyLocationError {
    #[error("retired locations cannot be corrected; restore explicitly first")]
    Retired,
    #[error("location input revision exhausted")]
    RevisionExhausted,
}

impl PartyLocation {
    pub fn create(id: PartyLocationId, party_id: PartyId, content: PartyLocationContent) -> Self {
        Self {
            id,
            party_id,
            content,
            lifecycle: PartyLocationLifecycle::Active,
            input_revision: PartyLocationInputRevision::INITIAL,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(
        id: PartyLocationId,
        party_id: PartyId,
        content: PartyLocationContent,
        lifecycle: PartyLocationLifecycle,
        input_revision: PartyLocationInputRevision,
    ) -> Self {
        Self {
            id,
            party_id,
            content,
            lifecycle,
            input_revision,
        }
    }
    /// Explicit assertion that the correction describes the same physical site.
    /// Relocation must create a different aggregate and retire this one.
    pub fn correct_same_site(
        &mut self,
        content: PartyLocationContent,
        evidence_changed: bool,
    ) -> Result<ChangeOutcome, PartyLocationError> {
        if self.content == content && !evidence_changed {
            return Ok(ChangeOutcome::Unchanged);
        }
        if self.lifecycle == PartyLocationLifecycle::Retired {
            return Err(PartyLocationError::Retired);
        }
        if self.content.geography != content.geography
            || self.content.position != content.position
            || evidence_changed
        {
            if self.input_revision.into_inner() >= i64::MAX as u64 {
                return Err(PartyLocationError::RevisionExhausted);
            }
            self.input_revision = self.input_revision.next();
        }
        self.content = content;
        Ok(ChangeOutcome::Changed)
    }
    pub fn set_lifecycle(&mut self, lifecycle: PartyLocationLifecycle) -> ChangeOutcome {
        if self.lifecycle == lifecycle {
            ChangeOutcome::Unchanged
        } else {
            self.lifecycle = lifecycle;
            ChangeOutcome::Changed
        }
    }
    pub fn id(&self) -> PartyLocationId {
        self.id
    }
    pub fn party_id(&self) -> PartyId {
        self.party_id
    }
    pub fn content(&self) -> &PartyLocationContent {
        &self.content
    }
    pub fn lifecycle(&self) -> PartyLocationLifecycle {
        self.lifecycle
    }
    pub fn input_revision(&self) -> PartyLocationInputRevision {
        self.input_revision
    }
    pub fn eligible_for_assignment(&self) -> bool {
        self.lifecycle == PartyLocationLifecycle::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> PartyLocation {
        PartyLocation::create(
            PartyLocationId::new(),
            PartyId::new(),
            PartyLocationContent {
                label: PartyLocationLabel::new("Showroom".to_owned()).unwrap(),
                roles: BTreeSet::from([PartyLocationRole::BusinessPremises]),
                geography: None,
                position: None,
                disclosure: PartyLocationDisclosure::Private,
            },
        )
    }
    #[test]
    fn correction_keeps_identity_and_invalidates_only_changed_inputs() {
        let mut location = sample();
        let id = location.id();
        assert!(
            !location
                .correct_same_site(location.content().clone(), false)
                .unwrap()
                .changed()
        );
        let mut content = location.content().clone();
        content.label = PartyLocationLabel::new("Main showroom".to_owned()).unwrap();
        assert!(
            location
                .correct_same_site(content.clone(), false)
                .unwrap()
                .changed()
        );
        assert_eq!(1, location.input_revision().into_inner());
        content.geography =
            GeographicDescription::new(None, Some(geo::CountryCode::DEU), None, None).unwrap();
        assert!(
            location
                .correct_same_site(content.clone(), false)
                .unwrap()
                .changed()
        );
        assert_eq!(2, location.input_revision().into_inner());
        assert!(location.correct_same_site(content, true).unwrap().changed());
        assert_eq!(3, location.input_revision().into_inner());
        assert_eq!(id, location.id());
    }
    #[test]
    fn retirement_restore_and_relocation_keep_independent_identities() {
        let mut old = sample();
        assert!(old.eligible_for_assignment());
        let new = PartyLocation::create(
            PartyLocationId::new(),
            old.party_id(),
            old.content().clone(),
        );
        assert_ne!(old.id(), new.id());
        assert!(old.set_lifecycle(PartyLocationLifecycle::Retired).changed());
        assert!(!old.eligible_for_assignment());
        assert!(!old.set_lifecycle(PartyLocationLifecycle::Retired).changed());
        let mut c = old.content().clone();
        c.label = PartyLocationLabel::new("Renamed".to_owned()).unwrap();
        assert_eq!(
            Err(PartyLocationError::Retired),
            old.correct_same_site(c, false)
        );
        assert!(old.set_lifecycle(PartyLocationLifecycle::Active).changed());
        assert!(old.eligible_for_assignment());
        assert_eq!(1, old.input_revision().into_inner());
    }
    #[test]
    fn labels_reject_blank_controls_and_oversized_unicode() {
        for s in [
            "",
            " \u{2003}",
            "x\nx",
            "x\0x",
            "x\u{2028}x",
            "x\u{2029}x",
            &"é".repeat(128),
        ] {
            assert!(PartyLocationLabel::new(s.to_owned()).is_err());
        }
        assert_eq!(
            "Store",
            PartyLocationLabel::new(" Store ".to_owned())
                .unwrap()
                .as_str()
        );
    }
}
