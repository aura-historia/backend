use application::error::box_error;
use listing_source_core::{
    AcquisitionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
};
use partnership_core::{
    partnership_application::{
        PartnershipApplication, PartnershipProposal, ProposedListingSource, ProposedParty,
        RehydratedPartnershipApplicationState,
    },
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
};
use partnership_service::ports::{
    PartnershipApplicationStorageVersion, PartnershipApplicationView,
    VersionedPartnershipApplication,
};
use party_core::{party::PartyContact, party_name::PartyName};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use strum::IntoEnumIterator;
use url::Url;
use user_core::user_id::UserId;

#[derive(sqlx::FromRow)]
pub(crate) struct ApplicationRow {
    pub(crate) partnership_application_id: uuid::Uuid,
    pub(crate) applicant_user_id: uuid::Uuid,
    pub(crate) business_state: String,
    pub(crate) proposal: serde_json::Value,
    pub(crate) version: i64,
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum MappingError {
    #[error("invalid application state")]
    State,
    #[error("invalid application proposal")]
    Proposal(#[source] serde_json::Error),
    #[error("invalid proposal value")]
    Value,
    #[error("invalid application version")]
    Version,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum ProposalV1 {
    ExistingListingSource {
        listing_source_id: uuid::Uuid,
    },
    ProposedListingSource {
        party: ProposedPartyV1,
        listing_source: ProposedListingSourceV1,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposedPartyV1 {
    name: String,
    phone: Option<String>,
    email: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposedListingSourceV1 {
    name: String,
    url: Option<String>,
    image: Option<String>,
    requested_acquisition_methods: Vec<String>,
}
impl From<&PartnershipProposal> for ProposalV1 {
    fn from(v: &PartnershipProposal) -> Self {
        match v {
            PartnershipProposal::ExistingListingSource { listing_source_id } => {
                Self::ExistingListingSource {
                    listing_source_id: (*listing_source_id).into(),
                }
            }
            PartnershipProposal::ProposedListingSource {
                party,
                listing_source,
            } => Self::ProposedListingSource {
                party: ProposedPartyV1 {
                    name: party.name.to_string(),
                    phone: party.contact.phone.clone(),
                    email: party.contact.email.as_ref().map(ToString::to_string),
                },
                listing_source: ProposedListingSourceV1 {
                    name: listing_source.name.to_string(),
                    url: listing_source
                        .presentation
                        .url
                        .as_ref()
                        .map(ToString::to_string),
                    image: listing_source
                        .presentation
                        .image
                        .as_ref()
                        .map(ToString::to_string),
                    requested_acquisition_methods: listing_source
                        .requested_acquisition_methods
                        .iter()
                        .map(|m| m.as_str().to_owned())
                        .collect(),
                },
            },
        }
    }
}
impl TryFrom<ProposalV1> for PartnershipProposal {
    type Error = MappingError;
    fn try_from(v: ProposalV1) -> Result<Self, Self::Error> {
        match v {
            ProposalV1::ExistingListingSource { listing_source_id } => {
                Ok(Self::ExistingListingSource {
                    listing_source_id: ListingSourceId::from(listing_source_id),
                })
            }
            ProposalV1::ProposedListingSource {
                party,
                listing_source,
            } => {
                let email = party
                    .email
                    .map(|v| v.parse::<Email>().map_err(|_| MappingError::Value))
                    .transpose()?;
                let methods = listing_source
                    .requested_acquisition_methods
                    .into_iter()
                    .map(|v| {
                        AcquisitionMethod::iter()
                            .find(|m| m.as_str() == v)
                            .ok_or(MappingError::Value)
                    })
                    .collect::<Result<HashSet<_>, _>>()?;
                Ok(Self::ProposedListingSource {
                    party: ProposedParty {
                        name: PartyName::from(party.name),
                        contact: PartyContact {
                            phone: party.phone,
                            email,
                        },
                    },
                    listing_source: ProposedListingSource {
                        name: ListingSourceName::from(listing_source.name),
                        presentation: ListingSourcePresentation {
                            url: listing_source
                                .url
                                .map(|v| Url::parse(&v).map_err(|_| MappingError::Value))
                                .transpose()?,
                            image: listing_source
                                .image
                                .map(|v| Url::parse(&v).map_err(|_| MappingError::Value))
                                .transpose()?,
                        },
                        requested_acquisition_methods: methods,
                    },
                })
            }
        }
    }
}
pub(crate) fn proposal_json(
    proposal: &PartnershipProposal,
) -> Result<serde_json::Value, MappingError> {
    serde_json::to_value(ProposalV1::from(proposal)).map_err(MappingError::Proposal)
}
pub(crate) fn application(
    row: ApplicationRow,
) -> Result<VersionedPartnershipApplication, MappingError> {
    let state =
        PartnershipApplicationState::from_code(&row.business_state).ok_or(MappingError::State)?;
    let proposal = serde_json::from_value::<ProposalV1>(row.proposal)
        .map_err(MappingError::Proposal)?
        .try_into()?;
    let version = PartnershipApplicationStorageVersion::try_from(row.version)
        .map_err(|_| MappingError::Version)?;
    Ok(domain_primitives::versioned::Versioned::new(
        PartnershipApplication::rehydrate(RehydratedPartnershipApplicationState {
            id: PartnershipApplicationId::from(row.partnership_application_id),
            applicant_user_id: UserId::from(row.applicant_user_id),
            state,
            proposal,
        }),
        version,
    ))
}
pub(crate) fn view(row: ApplicationRow) -> Result<PartnershipApplicationView, MappingError> {
    let app = application(row)?.value;
    Ok(PartnershipApplicationView {
        id: app.id(),
        applicant_user_id: app.applicant_user_id(),
        state: app.state(),
        proposal: app.proposal().clone(),
    })
}
pub(crate) fn invalid(
    error: MappingError,
) -> partnership_service::ports::PartnershipApplicationRepositoryError {
    partnership_service::ports::PartnershipApplicationRepositoryError::InvalidPersistedState {
        source: box_error(error),
    }
}
