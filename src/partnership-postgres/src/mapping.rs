use application::error::box_error;
use listing_source_core::{
    ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
};
use partnership_core::{
    partnership_application::{
        PartnershipApplication, PartnershipApplicationApprovalResult, PartnershipProposal,
        ProposedListingSource, ProposedParty, RehydratedPartnershipApplicationState,
    },
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
    partnership_id::PartnershipId,
};
use partnership_service::ports::{
    PartnershipApplicationStorageVersion, PartnershipApplicationView,
    VersionedPartnershipApplication,
};
use partnership_service::use_cases::queries::list_admin_partnership_applications::AdminPartnershipApplicationSummary;
use party_core::{party::PartyContact, party_name::PartyName};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use url::Url;
use user_core::user_id::UserId;

pub(crate) const APPLICATION_COLUMNS: &str = "partnership_application_id, applicant_user_id, business_state, proposal, approved_partnership_id, approved_listing_source_id, version, created, updated";

#[derive(sqlx::FromRow)]
pub(crate) struct ApplicationRow {
    pub(crate) partnership_application_id: uuid::Uuid,
    pub(crate) applicant_user_id: uuid::Uuid,
    pub(crate) business_state: String,
    pub(crate) proposal: serde_json::Value,
    pub(crate) approved_partnership_id: Option<uuid::Uuid>,
    pub(crate) approved_listing_source_id: Option<uuid::Uuid>,
    pub(crate) version: i64,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum MappingError {
    #[error("invalid application state")]
    State,
    #[error("invalid application proposal")]
    Proposal(#[source] serde_json::Error),
    #[error("invalid proposal value")]
    Value,
    #[error("invalid application approval result")]
    ApprovalResult,
    #[error("inconsistent application approval result")]
    Rehydration(
        #[source] partnership_core::partnership_application::RehydratedPartnershipApplicationError,
    ),
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
    requested_ingestion_methods: Vec<String>,
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
                    requested_ingestion_methods: listing_source
                        .requested_ingestion_methods
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
                    .requested_ingestion_methods
                    .into_iter()
                    .map(|v| {
                        ListingIngestionMethod::iter()
                            .find(|m| m.as_str() == v)
                            .ok_or(MappingError::Value)
                    })
                    .collect::<Result<HashSet<_>, _>>()?;
                Ok(Self::ProposedListingSource {
                    party: ProposedParty {
                        name: PartyName::try_from(party.name).map_err(|_| MappingError::Value)?,
                        contact: PartyContact {
                            phone: party.phone,
                            email,
                        },
                    },
                    listing_source: ProposedListingSource {
                        name: ListingSourceName::try_from(listing_source.name)
                            .map_err(|_| MappingError::Value)?,
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
                        requested_ingestion_methods: methods,
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
fn application_values(
    row: &ApplicationRow,
) -> Result<
    (
        PartnershipApplicationState,
        PartnershipProposal,
        Option<PartnershipApplicationApprovalResult>,
    ),
    MappingError,
> {
    let state =
        PartnershipApplicationState::from_code(&row.business_state).ok_or(MappingError::State)?;
    let proposal = serde_json::from_value::<ProposalV1>(row.proposal.clone())
        .map_err(MappingError::Proposal)?
        .try_into()?;
    let approval_result = match (row.approved_partnership_id, row.approved_listing_source_id) {
        (Some(partnership_id), Some(listing_source_id)) => {
            Some(PartnershipApplicationApprovalResult::new(
                PartnershipId::from(partnership_id),
                ListingSourceId::from(listing_source_id),
            ))
        }
        (None, None) => None,
        _ => return Err(MappingError::ApprovalResult),
    };
    Ok((state, proposal, approval_result))
}

fn rehydrate_application(
    row: &ApplicationRow,
    state: PartnershipApplicationState,
    proposal: PartnershipProposal,
    approval_result: Option<PartnershipApplicationApprovalResult>,
) -> Result<PartnershipApplication, MappingError> {
    PartnershipApplication::rehydrate(RehydratedPartnershipApplicationState {
        id: PartnershipApplicationId::from(row.partnership_application_id),
        applicant_user_id: UserId::from(row.applicant_user_id),
        state,
        proposal,
        approval_result,
    })
    .map_err(MappingError::Rehydration)
}

pub(crate) fn application(
    row: ApplicationRow,
) -> Result<VersionedPartnershipApplication, MappingError> {
    let (state, proposal, approval_result) = application_values(&row)?;
    let version = PartnershipApplicationStorageVersion::try_from(row.version)
        .map_err(|_| MappingError::Version)?;
    let application = rehydrate_application(&row, state, proposal, approval_result)?;
    Ok(domain_primitives::versioned::Versioned::new(
        application,
        version,
    ))
}

pub(crate) fn admin_summary(
    row: ApplicationRow,
) -> Result<AdminPartnershipApplicationSummary, MappingError> {
    let (state, proposal, approval_result) = application_values(&row)?;
    PartnershipApplicationStorageVersion::try_from(row.version)
        .map_err(|_| MappingError::Version)?;
    let application = rehydrate_application(&row, state, proposal, approval_result)?;
    let (approved_partnership_id, approved_listing_source_id) = application
        .approval_result()
        .map(|result| {
            (
                Some(result.partnership_id()),
                Some(result.listing_source_id()),
            )
        })
        .unwrap_or((None, None));

    Ok(AdminPartnershipApplicationSummary {
        id: application.id(),
        applicant_user_id: application.applicant_user_id(),
        state: application.state(),
        proposal: application.proposal().clone(),
        approved_partnership_id,
        approved_listing_source_id,
        created: row.created,
        updated: row.updated,
    })
}
pub(crate) fn view(row: ApplicationRow) -> Result<PartnershipApplicationView, MappingError> {
    let app = application(row)?.value;
    Ok(PartnershipApplicationView {
        id: app.id(),
        applicant_user_id: app.applicant_user_id(),
        state: app.state(),
        proposal: app.proposal().clone(),
        approval_result: app.approval_result(),
    })
}
pub(crate) fn invalid(
    error: MappingError,
) -> partnership_service::ports::PartnershipApplicationRepositoryError {
    partnership_service::ports::PartnershipApplicationRepositoryError::InvalidPersistedState {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    fn row(state: &str, proposal: serde_json::Value) -> ApplicationRow {
        ApplicationRow {
            partnership_application_id: uuid::Uuid::new_v4(),
            applicant_user_id: uuid::Uuid::new_v4(),
            business_state: state.to_owned(),
            proposal,
            approved_partnership_id: None,
            approved_listing_source_id: None,
            version: 1,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-02 00:00 UTC),
        }
    }

    fn existing_proposal() -> serde_json::Value {
        json!({
            "type": "EXISTING_LISTING_SOURCE",
            "listing_source_id": uuid::Uuid::new_v4(),
        })
    }

    #[test]
    fn should_map_admin_summary_with_persisted_metadata() {
        let row = row("SUBMITTED", existing_proposal());
        let expected_id = PartnershipApplicationId::from(row.partnership_application_id);
        let expected_applicant = UserId::from(row.applicant_user_id);

        let summary = admin_summary(row)
            .unwrap_or_else(|error| panic!("valid application row should map: {error}"));

        assert_eq!(expected_id, summary.id);
        assert_eq!(expected_applicant, summary.applicant_user_id);
        assert_eq!(datetime!(2026-01-01 00:00 UTC), summary.created);
        assert_eq!(datetime!(2026-01-02 00:00 UTC), summary.updated);
        assert_eq!(None, summary.approved_partnership_id);
        assert_eq!(None, summary.approved_listing_source_id);
    }

    #[test]
    fn should_map_view_with_persisted_approval_result() {
        let mut row = row("APPROVED", existing_proposal());
        let partnership_id = uuid::Uuid::new_v4();
        let listing_source_id = uuid::Uuid::new_v4();
        row.approved_partnership_id = Some(partnership_id);
        row.approved_listing_source_id = Some(listing_source_id);

        let view =
            view(row).unwrap_or_else(|error| panic!("valid application row should map: {error}"));

        assert_eq!(
            Some(PartnershipApplicationApprovalResult::new(
                PartnershipId::from(partnership_id),
                ListingSourceId::from(listing_source_id),
            )),
            view.approval_result
        );
    }

    #[test]
    fn should_reject_invalid_persisted_values_for_admin_summary() {
        assert!(matches!(
            admin_summary(row("NOT_A_STATE", existing_proposal())),
            Err(MappingError::State)
        ));
        assert!(matches!(
            admin_summary(row("SUBMITTED", json!({"type": "UNKNOWN"}))),
            Err(MappingError::Proposal(_))
        ));

        let mut invalid_version = row("SUBMITTED", existing_proposal());
        invalid_version.version = 0;
        assert!(matches!(
            admin_summary(invalid_version),
            Err(MappingError::Version)
        ));
    }
}
