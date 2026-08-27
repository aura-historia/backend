use crate::{
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
};
use listing_source_core::{
    AcquisitionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
};
use party_core::{party::PartyContact, party_name::PartyName};
use std::collections::HashSet;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct PartnershipApplication {
    id: PartnershipApplicationId,
    applicant_user_id: UserId,
    state: PartnershipApplicationState,
    proposal: PartnershipProposal,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PartnershipProposal {
    ExistingListingSource {
        listing_source_id: ListingSourceId,
    },
    ProposedListingSource {
        party: ProposedParty,
        listing_source: ProposedListingSource,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposedParty {
    pub name: PartyName,
    pub contact: PartyContact,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedListingSource {
    pub name: ListingSourceName,
    pub presentation: ListingSourcePresentation,
    pub requested_acquisition_methods: HashSet<AcquisitionMethod>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NewPartnershipApplication {
    pub id: PartnershipApplicationId,
    pub applicant_user_id: UserId,
    pub proposal: PartnershipProposal,
}
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedPartnershipApplicationState {
    pub id: PartnershipApplicationId,
    pub applicant_user_id: UserId,
    pub state: PartnershipApplicationState,
    pub proposal: PartnershipProposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot transition partnership application from {from:?} to {to:?}")]
pub struct PartnershipApplicationTransitionError {
    pub from: PartnershipApplicationState,
    pub to: PartnershipApplicationState,
}

impl PartnershipApplication {
    pub fn submit(input: NewPartnershipApplication) -> Self {
        Self {
            id: input.id,
            applicant_user_id: input.applicant_user_id,
            state: PartnershipApplicationState::Submitted,
            proposal: input.proposal,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartnershipApplicationState) -> Self {
        Self {
            id: state.id,
            applicant_user_id: state.applicant_user_id,
            state: state.state,
            proposal: state.proposal,
        }
    }
    pub fn mark_in_review(&mut self) -> Result<(), PartnershipApplicationTransitionError> {
        self.transition(PartnershipApplicationState::InReview)
    }
    pub fn approve(&mut self) -> Result<(), PartnershipApplicationTransitionError> {
        self.transition(PartnershipApplicationState::Approved)
    }
    pub fn reject(&mut self) -> Result<(), PartnershipApplicationTransitionError> {
        self.transition(PartnershipApplicationState::Rejected)
    }
    pub fn withdraw(&mut self) -> Result<(), PartnershipApplicationTransitionError> {
        self.transition(PartnershipApplicationState::Withdrawn)
    }
    pub fn id(&self) -> PartnershipApplicationId {
        self.id
    }
    pub fn applicant_user_id(&self) -> UserId {
        self.applicant_user_id
    }
    pub fn state(&self) -> PartnershipApplicationState {
        self.state
    }
    pub fn proposal(&self) -> &PartnershipProposal {
        &self.proposal
    }
    fn transition(
        &mut self,
        to: PartnershipApplicationState,
    ) -> Result<(), PartnershipApplicationTransitionError> {
        let valid = matches!(
            (self.state, to),
            (
                PartnershipApplicationState::Submitted,
                PartnershipApplicationState::InReview | PartnershipApplicationState::Withdrawn
            ) | (
                PartnershipApplicationState::InReview,
                PartnershipApplicationState::Approved
                    | PartnershipApplicationState::Rejected
                    | PartnershipApplicationState::Withdrawn
            )
        );
        if !valid {
            return Err(PartnershipApplicationTransitionError {
                from: self.state,
                to,
            });
        }
        self.state = to;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn application() -> PartnershipApplication {
        PartnershipApplication::submit(NewPartnershipApplication {
            id: PartnershipApplicationId::new(),
            applicant_user_id: UserId::new(),
            proposal: PartnershipProposal::ExistingListingSource {
                listing_source_id: ListingSourceId::new(),
            },
        })
    }
    #[test]
    fn should_allow_exact_lifecycle() {
        let mut application = application();
        assert_eq!(Ok(()), application.mark_in_review());
        assert_eq!(Ok(()), application.approve());
        assert_eq!(PartnershipApplicationState::Approved, application.state());
    }
    #[test]
    fn should_allow_withdrawal_only_before_terminal_state() {
        let mut application = application();
        assert_eq!(Ok(()), application.withdraw());
        assert!(application.mark_in_review().is_err());
    }
    #[test]
    fn should_reject_direct_decision() {
        assert!(application().approve().is_err());
    }
}
