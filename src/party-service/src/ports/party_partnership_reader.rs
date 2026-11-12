use application::error::BoxError;
use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;

#[derive(Debug, thiserror::Error)]
pub enum PartyPartnershipReadError {
    #[error("temporary party partnership read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid party partnership read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

/// Reads the Partnership a Party currently holds, for presentation beside the Party.
#[async_trait::async_trait]
pub trait PartyPartnershipReader: Send {
    /// The Party's `ACTIVE` Partnership; `None` when it has none or it was dissolved.
    async fn find_active_partnership_id(
        &mut self,
        party_id: PartyId,
    ) -> Result<Option<PartnershipId>, PartyPartnershipReadError>;
}

pub trait PartyPartnershipReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyPartnershipReader + 'tx;
}
