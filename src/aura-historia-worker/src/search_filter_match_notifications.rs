#[cfg(test)]
use crate::cdc::{DomainJob, DomainJobPayload};
use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_match_notification_lambda::execute_job;
pub use search_filter_match_notification_lambda::{
    SearchFilterMatchNotificationJobDisposition, process_search_filter_match_notification_job,
};
#[cfg(test)]
use search_filter_match_notification_lambda::{command_from_job, notification_outcome};
use search_filter_service::use_cases::GenerateSearchFilterMatchNotificationUseCase;
#[cfg(test)]
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationResult,
};
use std::sync::Arc;

pub async fn consume_search_filter_match_notification_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterMatchNotification, move |job| {
            let use_case = Arc::clone(&use_case);
            async move { polling_outcome(execute_job(use_case.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: SearchFilterMatchNotificationJobDisposition) -> JobOutcome {
    match disposition {
        SearchFilterMatchNotificationJobDisposition::Complete(category) => {
            JobOutcome::Complete(category)
        }
        SearchFilterMatchNotificationJobDisposition::Retry(category) => JobOutcome::Retry(category),
        SearchFilterMatchNotificationJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        SearchFilterMatchNotificationJobDisposition::Poison(category) => {
            JobOutcome::Invalid(category)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, SearchFilterMatchCreatedJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use user_core::user_id::UserId;
    use uuid::Uuid;

    #[test]
    fn should_map_typed_match_job_to_notification_command() -> Result<(), Box<dyn std::error::Error>>
    {
        let user_id = UserId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001))?;
        let search_filter_id = UserSearchFilterId::try_from(Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0006,
        ))?;
        let product_listing_id =
            ProductListingId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0003))?;
        let origin_event_id =
            EventId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0004))?;

        assert_eq!("usr_01j0000000e008000000000001", user_id.to_string());
        assert_eq!(
            "sf_01j0000000e008000000000006",
            search_filter_id.to_string()
        );
        assert_eq!(
            "pl_01j0000000e008000000000003",
            product_listing_id.to_string()
        );
        assert_eq!(
            "evt_01j0000000e008000000000004",
            origin_event_id.to_string()
        );

        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::SearchFilterMatchNotification,
            idempotency_key: IdempotencyKey::new(format!(
                "search-filter-match:{user_id}:{search_filter_id}:{product_listing_id}:{origin_event_id}"
            )),
            ordering_key: OrderingKey::new(format!("user:{user_id}")),
            payload: DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
                user_id,
                user_search_filter_id: search_filter_id,
                product_listing_id,
                origin_event_id,
            }),
        });

        assert_eq!(
            Ok(GenerateSearchFilterMatchNotificationCommand {
                user_id,
                search_filter_id,
                product_listing_id,
                origin_event_id,
            }),
            command
        );
        Ok(())
    }

    #[test]
    fn should_retain_missing_historical_facts_and_complete_semantic_suppression() {
        use GenerateSearchFilterMatchNotificationResult as R;
        for result in [
            R::SuppressedForMissingMatch,
            R::SuppressedForMissingProductListing,
        ] {
            assert!(matches!(
                notification_outcome(result),
                SearchFilterMatchNotificationJobDisposition::Retry(_)
            ));
        }
        for result in [
            R::Created,
            R::AlreadyExists,
            R::SuppressedByQuota,
            R::SuppressedForMissingUser,
            R::SuppressedForWithdrawnProductListing,
            R::SuppressedForStaleMatch,
        ] {
            assert!(matches!(
                notification_outcome(result),
                SearchFilterMatchNotificationJobDisposition::Complete(_)
            ));
        }
    }
}
