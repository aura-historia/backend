mod mapping;
mod readers;
mod repositories;

pub use readers::{SqlxListingSourceAuthorization, SqlxPartnershipApplicationReaderFactory};
pub use repositories::{
    SqlxListingSourceGrantRepositoryFactory, SqlxPartnershipApplicationRepositoryFactory,
    SqlxPartnershipRepositoryFactory,
};

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use listing_source_postgres::SqlxListingSourceRepositoryFactory;
    use notification_service::ports::notification_creator::{
        NewNotification, NotificationCreationError, NotificationCreationOutcome,
        NotificationCreator, NotificationCreatorFactory,
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use partnership_service::{
        ports::ListingSourceAuthorization,
        use_cases::commands::approve_partnership_application::{
            ApprovePartnershipApplicationCommand, ApprovePartnershipApplicationHandler,
            ApprovePartnershipApplicationUseCase,
        },
    };
    use party_postgres::SqlxPartyRepositoryFactory;
    use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
    use serde_json::json;
    use sqlx::PgPool;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use user_core::user_id::UserId;
    use user_postgres::SqlxUserAdminReaderFactory;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[derive(Clone)]
    struct TestNotificationCreatorFactory;

    struct TestNotificationCreator;

    impl NotificationCreatorFactory<SqlxTransaction> for TestNotificationCreatorFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut SqlxTransaction,
        ) -> impl NotificationCreator + 'tx {
            TestNotificationCreator
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for TestNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            Ok(notifications
                .iter()
                .map(|_| NotificationCreationOutcome::Duplicate)
                .collect())
        }
    }

    fn handler(
        pool: PgPool,
    ) -> ApprovePartnershipApplicationHandler<
        SqlxUnitOfWork,
        SqlxPartnershipApplicationRepositoryFactory,
        SqlxPartyRepositoryFactory,
        SqlxListingSourceRepositoryFactory,
        SqlxPartnershipRepositoryFactory,
        SqlxPartnershipRepositoryFactory,
        SqlxListingSourceGrantRepositoryFactory,
        SqlxUserAdminReaderFactory,
        TestNotificationCreatorFactory,
    > {
        ApprovePartnershipApplicationHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxPartnershipApplicationRepositoryFactory::new(),
            SqlxPartyRepositoryFactory::new(),
            SqlxListingSourceRepositoryFactory::new(),
            SqlxPartnershipRepositoryFactory::new(),
            SqlxPartnershipRepositoryFactory::new(),
            SqlxListingSourceGrantRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
            TestNotificationCreatorFactory,
        )
    }

    async fn seed_user(pool: &PgPool) -> UserId {
        let user_id = UserId::new();
        let result = sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{user_id}@example.test"))
        .execute(pool)
        .await;
        assert!(result.is_ok());
        user_id
    }

    async fn seed_application(
        pool: &PgPool,
        applicant_user_id: UserId,
        state: &str,
        proposal: serde_json::Value,
    ) -> PartnershipApplicationId {
        let application_id = PartnershipApplicationId::new();
        let result = sqlx::query(
            "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal) VALUES ($1, $2, $3, $4)",
        )
        .bind(uuid::Uuid::from(application_id))
        .bind(uuid::Uuid::from(applicant_user_id))
        .bind(state)
        .bind(proposal)
        .execute(pool)
        .await;
        assert!(result.is_ok());
        application_id
    }

    async fn count(pool: &PgPool, table: &str) -> i64 {
        let result = match table {
            "parties" => {
                sqlx::query_scalar("SELECT count(*) FROM parties")
                    .fetch_one(pool)
                    .await
            }
            "listing_sources" => {
                sqlx::query_scalar("SELECT count(*) FROM listing_sources")
                    .fetch_one(pool)
                    .await
            }
            "partnerships" => {
                sqlx::query_scalar("SELECT count(*) FROM partnerships")
                    .fetch_one(pool)
                    .await
            }
            "partnership_members" => {
                sqlx::query_scalar("SELECT count(*) FROM partnership_members")
                    .fetch_one(pool)
                    .await
            }
            "partnership_listing_source_grants" => {
                sqlx::query_scalar("SELECT count(*) FROM partnership_listing_source_grants")
                    .fetch_one(pool)
                    .await
            }
            _ => panic!("unsupported count table: {table}"),
        };
        match result {
            Ok(value) => value,
            Err(error) => panic!("count {table}: {error}"),
        }
    }

    async fn application_state(pool: &PgPool, application_id: PartnershipApplicationId) -> String {
        match sqlx::query_scalar::<_, String>(
            "SELECT business_state FROM partnership_applications WHERE partnership_application_id = $1",
        )
        .bind(uuid::Uuid::from(application_id))
        .fetch_one(pool)
        .await
        {
            Ok(state) => state,
            Err(error) => panic!("read application state: {error}"),
        }
    }

    fn proposed_source() -> serde_json::Value {
        json!({
            "type": "PROPOSED_LISTING_SOURCE",
            "party": { "name": "Northwind Antiques", "phone": null, "email": null },
            "listing_source": {
                "name": "Northwind Source",
                "url": null,
                "image": null,
                "requested_acquisition_methods": ["PARTNER_API"]
            }
        })
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_commit_proposed_source_approval_and_replay_without_duplicate_members_or_grants()
    {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let application_id = seed_application(&pool, user_id, "IN_REVIEW", proposed_source()).await;
        let handler = handler(pool.clone());

        let first = handler
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;
        assert!(first.is_ok());
        let second = handler
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;
        assert!(second.is_ok());

        assert_eq!("APPROVED", application_state(&pool, application_id).await);
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
        assert_eq!(1, count(&pool, "partnerships").await);
        assert_eq!(1, count(&pool, "partnership_members").await);
        assert_eq!(1, count(&pool, "partnership_listing_source_grants").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_approve_existing_source_and_expose_granted_source_authorization() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let party_id = uuid::Uuid::new_v4();
        let source_id = uuid::Uuid::new_v4();
        let party = sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'existing-operator', 'Existing Operator')",
        )
        .bind(party_id)
        .execute(&pool)
        .await;
        assert!(party.is_ok());
        let source = sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, 'existing-source', 'Existing Source', $2)",
        )
        .bind(source_id)
        .bind(party_id)
        .execute(&pool)
        .await;
        assert!(source.is_ok());
        let application_id = seed_application(
            &pool,
            user_id,
            "IN_REVIEW",
            json!({
                "type": "EXISTING_LISTING_SOURCE",
                "listing_source_id": source_id,
            }),
        )
        .await;

        let result = handler(pool.clone())
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;
        assert!(result.is_ok());
        let authorization = SqlxListingSourceAuthorization::new(pool.clone());
        let source_id = listing_source_core::ListingSourceId::from(source_id);
        assert!(matches!(
            authorization.can_write_source(user_id, source_id).await,
            Ok(true)
        ));
        let administered = authorization.list_sources_user_administers(user_id).await;
        assert!(matches!(administered, Ok(sources) if sources.len() == 1));
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_roll_back_proposed_approval_when_party_slug_conflicts() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let existing_party = sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'northwind-antiques', 'Existing Northwind')",
        )
        .bind(uuid::Uuid::new_v4())
        .execute(&pool)
        .await;
        assert!(existing_party.is_ok());
        let application_id = seed_application(&pool, user_id, "IN_REVIEW", proposed_source()).await;

        let result = handler(pool.clone())
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(partnership_service::use_cases::commands::approve_partnership_application::ApprovePartnershipApplicationError::SlugConflict { .. })
        ));
        assert_eq!("IN_REVIEW", application_state(&pool, application_id).await);
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(0, count(&pool, "listing_sources").await);
        assert_eq!(0, count(&pool, "partnerships").await);
        assert_eq!(0, count(&pool, "partnership_members").await);
        assert_eq!(0, count(&pool, "partnership_listing_source_grants").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_leave_submitted_application_unchanged_when_approval_is_invalid() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let application_id = seed_application(&pool, user_id, "SUBMITTED", proposed_source()).await;

        let result = handler(pool.clone())
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(partnership_service::use_cases::commands::approve_partnership_application::ApprovePartnershipApplicationError::ApplicationNotApprovable)
        ));
        assert_eq!("SUBMITTED", application_state(&pool, application_id).await);
        assert_eq!(0, count(&pool, "parties").await);
    }
}
