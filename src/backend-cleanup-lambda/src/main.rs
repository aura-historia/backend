use backend_cleanup_lambda::{
    cleanup_batch_size_from_env, run_cleanup, run_listing_geography_recovery,
    run_public_catalogue_overview_refresh, run_saved_search_digest_maintenance,
    run_saved_search_notification_recovery,
};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::resolution::MaintainResolutionsHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
    SqlxSavedSearchDigestStoreFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
    saved_search_digest::MaintainSavedSearchDigestsHandler,
};
use oauth_postgres::SqlxExpiredCredentialCleanup;
use oauth_service::use_cases::CleanupExpiredCredentialsAndProviderReceiptsHandler;
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::geography::SqlxGeographyProjectionStoreFactory;
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSnapshotReaderFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
    SqlxPublicCatalogueOverviewRefresherFactory,
};
use product_listing_service::geography::ProjectListingGeographyHandler;
use product_listing_service::use_cases::RefreshPublicCatalogueOverviewHandler;
use search_filter_postgres::{
    SqlxPendingSavedSearchNotificationReader, SqlxSearchFilterMatchNotificationSourceReaderFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationHandler, RecoverSavedSearchNotificationsHandler,
};
use serde_json::Value;
use std::{sync::Arc, time::Instant};
use user_postgres::SqlxUserTierEntitlementsFactory;
use user_postgres::{SqlxConsentWorkflowCleanup, SqlxNewsletterConfirmationChallengesRepository};
use user_service::ports::SystemNewsletterConfirmationClock;
use user_service::use_cases::CleanupConsentWorkflowHandler;

const COMPONENT: &str = "backend-cleanup-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let batch_size = cleanup_batch_size_from_env()?;
    let postgres = RuntimePostgresConfig::from_env()?;
    let cleanup = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(move |event: LambdaEvent<Value>| {
        let postgres = postgres.clone();
        let cleanup = Arc::clone(&cleanup);
        async move {
            log_invocation_start(COMPONENT, &event.context);
            let cleanup = cleanup
                .get_or_try_build(|| async {
                    let pool = postgres.connect(COMPONENT).await?;
                    Ok::<_, Error>(Arc::new((
                        CleanupExpiredCredentialsAndProviderReceiptsHandler::new(
                            SqlxExpiredCredentialCleanup::new(pool.clone()),
                        ),
                        CleanupConsentWorkflowHandler::new(
                            SqlxUnitOfWork::new(pool.clone()),
                            SqlxNewsletterConfirmationChallengesRepository::new(),
                            SqlxConsentWorkflowCleanup::new(),
                            SystemNewsletterConfirmationClock,
                        ),
                        MaintainResolutionsHandler::new(
                            SqlxUnitOfWork::new(pool.clone()),
                            SqlxResolutionRepositoryFactory,
                        ),
                        ProjectListingGeographyHandler::new(
                            SqlxUnitOfWork::new(pool.clone()),
                            SqlxGeographyProjectionStoreFactory,
                            SqlxResolutionRepositoryFactory,
                        ),
                        RecoverSavedSearchNotificationsHandler::new(
                            SqlxPendingSavedSearchNotificationReader::new(pool.clone()),
                            Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
                                SqlxUnitOfWork::new(pool.clone()),
                                SqlxSearchFilterMatchNotificationSourceReaderFactory,
                                SqlxProductListingSearchFilterMatchSourceReaderFactory,
                                SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
                                SqlxUserTierEntitlementsFactory::new(),
                                SqlxProductListingContentAssessmentSnapshotReaderFactory,
                                NotificationCreationCoordinatorFactory::new(
                                    SqlxNotificationRepositoryFactory,
                                    InitialExternalDeliveryPlanReaderFactory,
                                    SqlxNotificationDeliveryIntentRepositoryFactory,
                                ),
                                SqlxSavedSearchDigestStoreFactory,
                            )),
                        ),
                        MaintainSavedSearchDigestsHandler::new(
                            SqlxUnitOfWork::new(pool.clone()),
                            SqlxSavedSearchDigestStoreFactory,
                        ),
                        RefreshPublicCatalogueOverviewHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxPublicCatalogueOverviewRefresherFactory::new(),
                        ),
                    )))
                })
                .await?;

            let cleanup_result = run_cleanup(&cleanup.0, &cleanup.1, &cleanup.2, batch_size).await;
            let recovery_result = run_listing_geography_recovery(&cleanup.3).await;
            let notification_recovery =
                run_saved_search_notification_recovery(&cleanup.4, batch_size.get() as usize).await;
            let digest_recovery =
                run_saved_search_digest_maintenance(&cleanup.5, batch_size.get() as usize).await;
            // Last: when due, the full aggregate must not delay bounded maintenance work.
            let overview_refresh = run_public_catalogue_overview_refresh(&cleanup.6).await;
            cleanup_result?;
            notification_recovery?;
            digest_recovery?;
            recovery_result?;
            overview_refresh?;
            Ok::<(), Error>(())
        }
    }))
    .await
}
