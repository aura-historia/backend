use application::error::box_error;
use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;
use party_service::ports::{
    PartyPartnershipReadError, PartyPartnershipReader, PartyPartnershipReaderFactory,
};
use platform_postgres::SqlxTransaction;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyPartnershipReaderFactory;

struct SqlxPartyPartnershipReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartyPartnershipReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartyPartnershipReaderFactory<SqlxTransaction> for SqlxPartyPartnershipReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyPartnershipReader + 'tx {
        SqlxPartyPartnershipReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartyPartnershipReader for SqlxPartyPartnershipReader<'_> {
    async fn find_active_partnership_id(
        &mut self,
        party_id: PartyId,
    ) -> Result<Option<PartnershipId>, PartyPartnershipReadError> {
        let partnership_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT partnership_id FROM partnerships WHERE party_id = $1 AND business_state = 'ACTIVE'",
        )
        .bind(party_id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| PartyPartnershipReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
        partnership_id
            .map(PartnershipId::try_from)
            .transpose()
            .map_err(|source| PartyPartnershipReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}
