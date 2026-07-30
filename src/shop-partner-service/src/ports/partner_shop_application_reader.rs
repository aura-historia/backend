#![allow(dead_code)]

use crate::ports::{PartnerShopApplicationRepositoryError, VersionedPartnerShopApplication};
use common::user_id::UserId;

#[async_trait::async_trait]
pub trait PartnerShopApplicationReader: Send {
    async fn list_all(
        &mut self,
    ) -> Result<Vec<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>;

    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>;
}

pub trait PartnerShopApplicationReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnerShopApplicationReader + 'tx;
}
