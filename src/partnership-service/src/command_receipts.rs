use crate::use_cases::commands::approve_partnership_application::ApprovePartnershipApplicationResult;
use crate::use_cases::commands::dissolve_partnership::{
    DissolvePartnershipOutcome, DissolvePartnershipResult,
};
use crate::use_cases::commands::grant_partnership_listing_source::{
    GrantPartnershipListingSourceOutcome, GrantPartnershipListingSourceResult,
};
use crate::use_cases::commands::grant_partnership_membership::{
    GrantPartnershipMembershipOutcome, GrantPartnershipMembershipResult,
};
use crate::use_cases::commands::mark_partnership_application_in_review::MarkPartnershipApplicationInReviewResult;
use crate::use_cases::commands::reject_partnership_application::RejectPartnershipApplicationResult;
use crate::use_cases::commands::revoke_partnership_listing_source::{
    RevokePartnershipListingSourceOutcome, RevokePartnershipListingSourceResult,
};
use crate::use_cases::commands::revoke_partnership_membership::{
    RevokePartnershipMembershipOutcome, RevokePartnershipMembershipResult,
};
use crate::use_cases::commands::submit_partnership_application::SubmitPartnershipApplicationResult;
use crate::use_cases::commands::withdraw_partnership_application::WithdrawPartnershipApplicationResult;
use application::{
    error::{BoxError, box_error, static_error},
    idempotency::{CommandOutcome, field},
};
use listing_source_core::{ListingSourceName, ListingSourcePresentation};
use partnership_core::{
    partnership_application::{
        PartnershipApplication, PartnershipApplicationApprovalResult, PartnershipProposal,
        ProposedListingSource, ProposedParty, RehydratedPartnershipApplicationState,
    },
    partnership_application_state::PartnershipApplicationState,
};
use party_core::{party::PartyContact, party_name::PartyName};
use serde_json::{Value, json};

pub(crate) fn proposal_value(proposal: &PartnershipProposal) -> Value {
    match proposal {
        PartnershipProposal::ExistingListingSource { listing_source_id } => {
            json!({"kind":"EXISTING","listing_source_id":listing_source_id})
        }
        PartnershipProposal::ProposedListingSource {
            party,
            listing_source,
        } => {
            let mut methods = listing_source
                .requested_ingestion_methods
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>();
            methods.sort_unstable();
            json!({"kind":"PROPOSED","party_name":party.name.as_ref(),"phone":party.contact.phone,"email":party.contact.email,"source_name":listing_source.name.as_ref(),"url":listing_source.presentation.url,"image":listing_source.presentation.image,"methods":methods})
        }
    }
}
fn read_proposal(v: Value) -> Result<PartnershipProposal, BoxError> {
    match field::<String>(&v, "kind")?.as_str() {
        "EXISTING" => Ok(PartnershipProposal::ExistingListingSource {
            listing_source_id: field(&v, "listing_source_id")?,
        }),
        "PROPOSED" => Ok(PartnershipProposal::ProposedListingSource {
            party: ProposedParty {
                name: PartyName::try_from(field::<String>(&v, "party_name")?).map_err(box_error)?,
                contact: PartyContact {
                    phone: field(&v, "phone")?,
                    email: field(&v, "email")?,
                },
            },
            listing_source: ProposedListingSource {
                name: ListingSourceName::try_from(field::<String>(&v, "source_name")?)
                    .map_err(box_error)?,
                presentation: ListingSourcePresentation {
                    url: field(&v, "url")?,
                    image: field(&v, "image")?,
                },
                requested_ingestion_methods: field::<Vec<String>>(&v, "methods")?
                    .into_iter()
                    .map(|v| v.parse())
                    .collect::<Result<_, _>>()
                    .map_err(box_error)?,
            },
        }),
        _ => Err(static_error("invalid partnership proposal receipt variant")),
    }
}
fn encode_application(application: &PartnershipApplication) -> Value {
    json!({"id":application.id(),"applicant":application.applicant_user_id(),"state":application.state().as_str(),"proposal":proposal_value(application.proposal()),"approval":application.approval_result().map(|v|json!({"partnership_id":v.partnership_id(),"listing_source_id":v.listing_source_id()}))})
}
fn decode_application(v: Value) -> Result<PartnershipApplication, BoxError> {
    let approval_result = field::<Option<Value>>(&v, "approval")?
        .map(|v| {
            Ok::<_, BoxError>(PartnershipApplicationApprovalResult::new(
                field(&v, "partnership_id")?,
                field(&v, "listing_source_id")?,
            ))
        })
        .transpose()?;
    PartnershipApplication::rehydrate(RehydratedPartnershipApplicationState {
        id: field(&v, "id")?,
        applicant_user_id: field(&v, "applicant")?,
        state: PartnershipApplicationState::from_code(&field::<String>(&v, "state")?)
            .ok_or_else(|| static_error("invalid application receipt state"))?,
        proposal: read_proposal(field(&v, "proposal")?)?,
        approval_result,
    })
    .map_err(box_error)
}
macro_rules! application_outcome {
    ($ty:ty) => {
        impl CommandOutcome for $ty {
            fn private_owner(&self) -> Option<user_core::user_id::UserId> {
                Some(self.application.applicant_user_id())
            }
            fn encode(&self) -> Result<Value, BoxError> {
                Ok(encode_application(&self.application))
            }
            fn decode(v: Value) -> Result<Self, BoxError> {
                Ok(Self {
                    application: decode_application(v)?,
                })
            }
        }
    };
}
application_outcome!(SubmitPartnershipApplicationResult);
application_outcome!(RejectPartnershipApplicationResult);
application_outcome!(WithdrawPartnershipApplicationResult);
application_outcome!(MarkPartnershipApplicationInReviewResult);
impl CommandOutcome for ApprovePartnershipApplicationResult {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.application.applicant_user_id())
    }
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"application":encode_application(&self.application),"partnership_id":self.partnership_id,"listing_source_id":self.listing_source_id}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            application: decode_application(field(&v, "application")?)?,
            partnership_id: field(&v, "partnership_id")?,
            listing_source_id: field(&v, "listing_source_id")?,
        })
    }
}
macro_rules! enum_outcome{($ty:ty,$outcome:ident,$($variant:ident => $code:literal),+)=>{impl CommandOutcome for $ty{
    fn encode(&self)->Result<Value,BoxError>{Ok(json!({"outcome":match self.outcome{$($outcome::$variant=>$code),+}}))}
    fn decode(v:Value)->Result<Self,BoxError>{let value=field::<String>(&v,"outcome")?;let outcome=match value.as_str(){$($code=>$outcome::$variant),+,_=>return Err(static_error("invalid relationship command outcome"))};Ok(Self{outcome})}
}}}
enum_outcome!(DissolvePartnershipResult,DissolvePartnershipOutcome,Dissolved=>"DISSOLVED",AlreadyDissolved=>"ALREADY_DISSOLVED");
enum_outcome!(GrantPartnershipMembershipResult,GrantPartnershipMembershipOutcome,Added=>"ADDED",AlreadyMember=>"ALREADY_MEMBER");
enum_outcome!(RevokePartnershipMembershipResult,RevokePartnershipMembershipOutcome,Removed=>"REMOVED",AlreadyAbsent=>"ALREADY_ABSENT");
enum_outcome!(GrantPartnershipListingSourceResult,GrantPartnershipListingSourceOutcome,Granted=>"GRANTED",AlreadyGranted=>"ALREADY_GRANTED");
enum_outcome!(RevokePartnershipListingSourceResult,RevokePartnershipListingSourceOutcome,Removed=>"REMOVED",AlreadyAbsent=>"ALREADY_ABSENT");

application::command_outcome!(crate::use_cases::commands::put_listing_source_ingestion_configuration::PutListingSourceIngestionConfigurationResult{created});
