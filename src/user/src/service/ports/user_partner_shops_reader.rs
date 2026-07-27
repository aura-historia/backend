#![allow(dead_code)]

use crate::service::use_cases::queries::list_partner_shops::{
    ListPartnerShopsRequest, ListPartnerShopsResult,
};

#[derive(Debug, thiserror::Error)]
pub enum UserPartnerShopsReadError {
    #[error("invalid user partner shops read model")]
    InvalidReadModel,
}

#[async_trait::async_trait]
pub(crate) trait UserPartnerShopsReader: Send + Sync {
    async fn list_partner_shops(
        &self,
        request: &ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, UserPartnerShopsReadError>;
}
