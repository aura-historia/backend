pub mod service;

use crate::service::PeriodicMatcherService;
use lambda_runtime::LambdaEvent;
use tracing::info;

#[tracing::instrument(skip(service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    service: &impl PeriodicMatcherService,
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    info!("Started hybrid-search search-filter product-matching.");
    let result = service.match_active_filters().await?;
    info!(
        filtersProcessed = result.filters_processed,
        matchesCreated = result.matches_created,
        notificationsCreated = result.notifications_created,
        "Finished hybrid-search search-filter product-matching."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{MockPeriodicMatcherService, PeriodicMatcherError, PeriodicMatcherResult};
    use lambda_runtime::Context;
    use product::service::query_service::SearchProductsError;

    fn event() -> LambdaEvent<serde_json::Value> {
        LambdaEvent::new(serde_json::json!({}), Context::default())
    }

    #[tokio::test]
    async fn should_succeed_when_service_succeeds() {
        let mut service = MockPeriodicMatcherService::default();
        service.expect_match_active_filters().return_once(|| {
            Box::pin(async {
                Ok(PeriodicMatcherResult {
                    filters_processed: 1,
                    matches_created: 2,
                    notifications_created: 3,
                })
            })
        });

        let result = handler(&service, event()).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_fail_when_service_fails() {
        let mut service = MockPeriodicMatcherService::default();
        service.expect_match_active_filters().return_once(|| {
            Box::pin(async {
                Err(PeriodicMatcherError::SearchProductsError(
                    SearchProductsError::OpenSearchError(
                        serde_json::Error::io(std::io::Error::other("boom")).into(),
                    ),
                ))
            })
        });

        let result = handler(&service, event()).await;

        assert!(result.is_err());
    }
}
