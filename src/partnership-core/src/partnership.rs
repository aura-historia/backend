use crate::partnership_id::PartnershipId;
use party_core::party_id::PartyId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partnership {
    id: PartnershipId,
    party_id: PartyId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPartnership {
    pub id: PartnershipId,
    pub party_id: PartyId,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RehydratedPartnershipState {
    pub id: PartnershipId,
    pub party_id: PartyId,
}

impl Partnership {
    pub fn create(input: NewPartnership) -> Self {
        Self {
            id: input.id,
            party_id: input.party_id,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartnershipState) -> Self {
        Self {
            id: state.id,
            party_id: state.party_id,
        }
    }
    pub fn id(&self) -> PartnershipId {
        self.id
    }
    pub fn party_id(&self) -> PartyId {
        self.party_id
    }
}
