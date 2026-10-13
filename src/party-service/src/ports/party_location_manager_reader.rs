use crate::use_cases::{
    party_locations::PartyLocationError,
    queries::list_party_location_managers::{
        ListPartyLocationManagersRequest, PartyLocationManagersPage,
    },
};

#[async_trait::async_trait]
pub trait PartyLocationManagerReader: Send {
    async fn list(
        &mut self,
        request: &ListPartyLocationManagersRequest,
    ) -> Result<PartyLocationManagersPage, PartyLocationError>;
}

pub trait PartyLocationManagerReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyLocationManagerReader + 'tx;
}
