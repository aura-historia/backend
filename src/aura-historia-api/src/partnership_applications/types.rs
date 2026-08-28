use listing_source_core::{
    AcquisitionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
};
use partnership_core::{
    partnership_application::{
        PartnershipApplication, PartnershipProposal, ProposedListingSource, ProposedParty,
    },
    partnership_application_state::PartnershipApplicationState,
};
use partnership_service::ports::PartnershipApplicationView;
use party_core::{party::PartyContact, party_name::PartyName};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use url::Url;

use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubmitPartnershipApplicationData {
    pub(super) proposal: PartnershipProposalData,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub(super) enum PartnershipProposalData {
    #[serde(rename = "EXISTING_LISTING_SOURCE")]
    ExistingListingSource { listing_source_id: ListingSourceId },
    #[serde(rename = "PROPOSED_LISTING_SOURCE")]
    ProposedListingSource {
        party: ProposedPartyData,
        listing_source: ProposedListingSourceData,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProposedPartyData {
    pub(super) name: String,
    #[serde(default)]
    pub(super) phone: Option<String>,
    #[serde(default)]
    pub(super) email: Option<Email>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProposedListingSourceData {
    pub(super) name: String,
    #[serde(default)]
    pub(super) url: Option<Url>,
    #[serde(default)]
    pub(super) image: Option<Url>,
    #[serde(with = "crate::wire::acquisition_method::set")]
    pub(super) requested_acquisition_methods: std::collections::HashSet<AcquisitionMethod>,
}

impl From<PartnershipProposalData> for PartnershipProposal {
    fn from(value: PartnershipProposalData) -> Self {
        match value {
            PartnershipProposalData::ExistingListingSource { listing_source_id } => {
                Self::ExistingListingSource { listing_source_id }
            }
            PartnershipProposalData::ProposedListingSource {
                party,
                listing_source,
            } => Self::ProposedListingSource {
                party: ProposedParty {
                    name: PartyName::from(party.name),
                    contact: PartyContact {
                        phone: party.phone,
                        email: party.email,
                    },
                },
                listing_source: ProposedListingSource {
                    name: ListingSourceName::from(listing_source.name),
                    presentation: ListingSourcePresentation {
                        url: listing_source.url,
                        image: listing_source.image,
                    },
                    requested_acquisition_methods: listing_source.requested_acquisition_methods,
                },
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OwnPartnershipApplicationData {
    id: Uuid,
    #[serde(with = "crate::wire::partnership_application_state")]
    state: PartnershipApplicationState,
    proposal: PartnershipProposalData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AdminPartnershipApplicationData {
    id: Uuid,
    applicant_user_id: Uuid,
    #[serde(with = "crate::wire::partnership_application_state")]
    state: PartnershipApplicationState,
    proposal: PartnershipProposalData,
}

impl From<PartnershipApplication> for OwnPartnershipApplicationData {
    fn from(value: PartnershipApplication) -> Self {
        Self {
            id: value.id().into(),
            state: value.state(),
            proposal: proposal_data(value.proposal()),
        }
    }
}

impl From<PartnershipApplicationView> for OwnPartnershipApplicationData {
    fn from(value: PartnershipApplicationView) -> Self {
        Self {
            id: value.id.into(),
            state: value.state,
            proposal: proposal_data(&value.proposal),
        }
    }
}

impl From<PartnershipApplication> for AdminPartnershipApplicationData {
    fn from(value: PartnershipApplication) -> Self {
        Self {
            id: value.id().into(),
            applicant_user_id: value.applicant_user_id().into(),
            state: value.state(),
            proposal: proposal_data(value.proposal()),
        }
    }
}

impl From<PartnershipApplicationView> for AdminPartnershipApplicationData {
    fn from(value: PartnershipApplicationView) -> Self {
        Self {
            id: value.id.into(),
            applicant_user_id: value.applicant_user_id.into(),
            state: value.state,
            proposal: proposal_data(&value.proposal),
        }
    }
}

fn proposal_data(value: &PartnershipProposal) -> PartnershipProposalData {
    match value {
        PartnershipProposal::ExistingListingSource { listing_source_id } => {
            PartnershipProposalData::ExistingListingSource {
                listing_source_id: *listing_source_id,
            }
        }
        PartnershipProposal::ProposedListingSource {
            party,
            listing_source,
        } => PartnershipProposalData::ProposedListingSource {
            party: ProposedPartyData {
                name: party.name.to_string(),
                phone: party.contact.phone.clone(),
                email: party.contact.email.clone(),
            },
            listing_source: ProposedListingSourceData {
                name: listing_source.name.to_string(),
                url: listing_source.presentation.url.clone(),
                image: listing_source.presentation.image.clone(),
                requested_acquisition_methods: listing_source.requested_acquisition_methods.clone(),
            },
        },
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum PartnershipApplicationDecisionData {
    Approve,
    Reject,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DecidePartnershipApplicationData {
    pub(super) decision: PartnershipApplicationDecisionData,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_deserialize_proposed_party_and_listing_source_without_shop_fields()
    -> Result<(), serde_json::Error> {
        let request: SubmitPartnershipApplicationData = serde_json::from_value(json!({
            "proposal": {
                "type": "PROPOSED_LISTING_SOURCE",
                "party": { "name": "Ada Antiques", "email": "ada@example.com" },
                "listingSource": {
                    "name": "Ada Antiques",
                    "url": "https://ada.example",
                    "requestedAcquisitionMethods": ["PARTNER_API"]
                }
            }
        }))?;

        let proposal: PartnershipProposal = request.proposal.into();
        assert!(matches!(
            proposal,
            PartnershipProposal::ProposedListingSource { .. }
        ));
        Ok(())
    }

    #[test]
    fn should_reject_legacy_shop_proposal_values() {
        let parsed = serde_json::from_value::<SubmitPartnershipApplicationData>(json!({
            "proposal": { "type": "EXISTING", "shopId": "00000000-0000-0000-0000-000000000001" }
        }));
        assert!(parsed.is_err());
    }
}
