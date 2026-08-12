use crate::mapping::{
    APPLICATION_COLUMNS, PartnerShopApplicationRow, bind_business_state, bind_execution_state,
    bind_payload_type, version_to_i64,
};
use common::error::boxed::box_error;
use common::postgres::SqlxTransaction;
use common::{partner_shop_application_id::PartnerShopApplicationId, user_id::UserId};
use shop_partner_core::partner_shop_application::PartnerShopApplication;
use shop_partner_service::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory, PartnerShopApplicationStorageVersion,
    VersionedPartnerShopApplication,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnerShopApplicationRepositoryFactory;

struct SqlxPartnerShopApplicationRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartnerShopApplicationRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnerShopApplicationRepositoryFactory<SqlxTransaction>
    for SqlxPartnerShopApplicationRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnerShopApplicationRepository + 'tx {
        SqlxPartnerShopApplicationRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnerShopApplicationRepository for SqlxPartnerShopApplicationRepository<'_> {
    async fn find_by_user_and_id(
        &mut self,
        user_id: UserId,
        id: PartnerShopApplicationId,
    ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
    {
        let sql = format!(
            "SELECT {} FROM partner_shop_applications WHERE applicant_user_id = $1 AND partner_shop_application_id = $2::uuid",
            APPLICATION_COLUMNS
        );
        let row = sqlx::query_as::<_, PartnerShopApplicationRow>(&sql)
            .bind(uuid::Uuid::from(user_id))
            .bind(id.to_string())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartnerShopApplicationLookupSqlxError)?;
        row.map(VersionedPartnerShopApplication::try_from)
            .transpose()
            .map_err(
                |source| PartnerShopApplicationRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                },
            )
    }

    async fn find_by_id(
        &mut self,
        id: PartnerShopApplicationId,
    ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
    {
        let sql = format!(
            "SELECT {} FROM partner_shop_applications WHERE partner_shop_application_id = $1::uuid",
            APPLICATION_COLUMNS
        );
        let row = sqlx::query_as::<_, PartnerShopApplicationRow>(&sql)
            .bind(id.to_string())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartnerShopApplicationLookupSqlxError)?;
        row.map(VersionedPartnerShopApplication::try_from)
            .transpose()
            .map_err(
                |source| PartnerShopApplicationRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                },
            )
    }

    async fn insert(
        &mut self,
        application: &PartnerShopApplication,
    ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError> {
        let sql = format!(
            r#"
            INSERT INTO partner_shop_applications (
                partner_shop_application_id, applicant_user_id, business_state, execution_state,
                payload_type, shop_id, task_token
            ) VALUES ($1::uuid, $2, $3, $4, $5, $6, $7)
            RETURNING {}
            "#,
            APPLICATION_COLUMNS
        );

        let row = sqlx::query_as::<_, PartnerShopApplicationRow>(&sql)
            .bind(application.id().to_string())
            .bind(uuid::Uuid::from(application.applicant_user_id()))
            .bind(bind_business_state(application.business_state()))
            .bind(bind_execution_state(application.execution_state()))
            .bind(bind_payload_type(application.payload()))
            .bind(uuid::Uuid::from(application.shop_id()))
            .bind(application.task_token())
            .fetch_one(&mut *self.connection)
            .await
            .map_err(PartnerShopApplicationWriteSqlxError)?;
        VersionedPartnerShopApplication::try_from(row).map_err(|source| {
            PartnerShopApplicationRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })
    }

    async fn update(
        &mut self,
        application: &PartnerShopApplication,
        expected_version: PartnerShopApplicationStorageVersion,
    ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError> {
        let sql = format!(
            r#"
            UPDATE partner_shop_applications
            SET business_state = $1,
                execution_state = $2,
                payload_type = $3,
                shop_id = $4,
                task_token = $5,
                version = version + 1,
                updated = now()
            WHERE partner_shop_application_id = $6::uuid AND version = $7
            RETURNING {}
            "#,
            APPLICATION_COLUMNS
        );

        let expected_version = version_to_i64(expected_version).map_err(|source| {
            PartnerShopApplicationRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        let row = sqlx::query_as::<_, PartnerShopApplicationRow>(&sql)
            .bind(bind_business_state(application.business_state()))
            .bind(bind_execution_state(application.execution_state()))
            .bind(bind_payload_type(application.payload()))
            .bind(uuid::Uuid::from(application.shop_id()))
            .bind(application.task_token())
            .bind(application.id().to_string())
            .bind(expected_version)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartnerShopApplicationWriteSqlxError)?
            .ok_or(PartnerShopApplicationRepositoryError::ConcurrencyConflict)?;
        VersionedPartnerShopApplication::try_from(row).map_err(|source| {
            PartnerShopApplicationRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })
    }

    async fn delete(
        &mut self,
        id: PartnerShopApplicationId,
        expected_version: PartnerShopApplicationStorageVersion,
    ) -> Result<(), PartnerShopApplicationRepositoryError> {
        let expected_version = version_to_i64(expected_version).map_err(|source| {
            PartnerShopApplicationRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        let result = sqlx::query("DELETE FROM partner_shop_applications WHERE partner_shop_application_id = $1::uuid AND version = $2")
            .bind(id.to_string())
            .bind(expected_version)
            .execute(&mut *self.connection)
            .await
            .map_err(PartnerShopApplicationWriteSqlxError)?;
        if result.rows_affected() == 0 {
            return Err(PartnerShopApplicationRepositoryError::ConcurrencyConflict);
        }
        Ok(())
    }
}

struct PartnerShopApplicationLookupSqlxError(sqlx::Error);
struct PartnerShopApplicationWriteSqlxError(sqlx::Error);

impl From<PartnerShopApplicationLookupSqlxError> for PartnerShopApplicationRepositoryError {
    fn from(error: PartnerShopApplicationLookupSqlxError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error.0),
        }
    }
}

impl From<PartnerShopApplicationWriteSqlxError> for PartnerShopApplicationRepositoryError {
    fn from(error: PartnerShopApplicationWriteSqlxError) -> Self {
        Self::Internal {
            source: box_error(error.0),
        }
    }
}
