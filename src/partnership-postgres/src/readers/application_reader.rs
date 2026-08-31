use crate::mapping::{ApplicationRow, view};
use application::error::box_error;
use partnership_service::ports::*;
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use user_core::user_id::UserId;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipApplicationReaderFactory;
struct Reader<'a> {
    connection: &'a mut PgConnection,
}
impl SqlxPartnershipApplicationReaderFactory {
    pub fn new() -> Self {
        Self
    }
}
impl PartnershipApplicationReaderFactory<SqlxTransaction>
    for SqlxPartnershipApplicationReaderFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl PartnershipApplicationReader + 'a {
        Reader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnershipApplicationReader for Reader<'_> {
    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError> {
        let rows=sqlx::query_as::<_,ApplicationRow>("SELECT partnership_application_id, applicant_user_id, business_state, proposal, approved_partnership_id, approved_listing_source_id, version FROM partnership_applications WHERE applicant_user_id=$1 ORDER BY created DESC").bind(uuid::Uuid::from(user_id)).fetch_all(&mut*self.connection).await.map_err(|e|PartnershipApplicationReadError::TemporarilyUnavailable{source:box_error(e)})?;
        rows.into_iter()
            .map(view)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PartnershipApplicationReadError::InvalidReadModel {
                source: box_error(e),
            })
    }
    async fn list_all(
        &mut self,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError> {
        let rows = sqlx::query_as::<_, ApplicationRow>("SELECT partnership_application_id, applicant_user_id, business_state, proposal, approved_partnership_id, approved_listing_source_id, version FROM partnership_applications ORDER BY created DESC")
        .fetch_all(&mut *self.connection)
        .await
        .map_err(
            |e| PartnershipApplicationReadError::TemporarilyUnavailable {
                source: box_error(e),
            },
        )?;
        rows.into_iter()
            .map(view)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PartnershipApplicationReadError::InvalidReadModel {
                source: box_error(e),
            })
    }
}
