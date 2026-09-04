use application::error::BoxError;
use partnership_core::{
    partnership_application::PartnershipProposal,
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
};
use user_core::user_id::UserId;

use crate::use_cases::queries::list_admin_partnership_applications::{
    ListAdminPartnershipApplicationsRequest, ListAdminPartnershipApplicationsResult,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PartnershipApplicationView {
    pub id: PartnershipApplicationId,
    pub applicant_user_id: UserId,
    pub state: PartnershipApplicationState,
    pub proposal: PartnershipProposal,
}

#[derive(Debug, thiserror::Error)]
pub enum PartnershipApplicationReadError {
    #[error("temporary partnership application read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partnership application read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership application read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnershipApplicationReader: Send {
    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError>;
    async fn search_admin(
        &mut self,
        request: &ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, PartnershipApplicationReadError>;
}

pub trait PartnershipApplicationReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnershipApplicationReader + 'tx;
}
