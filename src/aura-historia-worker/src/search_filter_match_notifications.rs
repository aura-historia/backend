use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationError,
    GenerateSearchFilterMatchNotificationResult, GenerateSearchFilterMatchNotificationUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for one saved-filter match notification job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterMatchNotificationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl SearchFilterMatchNotificationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

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

/// Decode and execute one compact schema-2 saved-filter match notification job.
///
/// This is transport-neutral so Lambda and the polling worker retain the same exact-match source,
/// lifecycle-lock, idempotency, and retry semantics.
pub async fn process_search_filter_match_notification_job(
    body: &str,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
) -> SearchFilterMatchNotificationJobDisposition {
    match crate::wire::decode(body, WorkerScope::SearchFilterMatchNotification) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => SearchFilterMatchNotificationJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
    job: DomainJob,
) -> SearchFilterMatchNotificationJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return SearchFilterMatchNotificationJobDisposition::Poison("match_metadata_invalid");
    };
    match use_case.execute(command).await {
        Ok(result) => notification_outcome(result),
        Err(error) => {
            use GenerateSearchFilterMatchNotificationError as E;
            match error {
                E::MatchSourceStateInvalid { .. }
                | E::ProductListingSourceStateInvalid { .. }
                | E::ProductListingSourceMismatch
                | E::ContentAssessmentStateInvalid { .. } => {
                    SearchFilterMatchNotificationJobDisposition::Poison(
                        "match_notification_state_invalid",
                    )
                }
                _ => SearchFilterMatchNotificationJobDisposition::DependencyUnavailable(
                    "match_notification_unavailable",
                ),
            }
        }
    }
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

fn notification_outcome(
    result: GenerateSearchFilterMatchNotificationResult,
) -> SearchFilterMatchNotificationJobDisposition {
    use GenerateSearchFilterMatchNotificationResult as R;
    match result {
        R::Created => SearchFilterMatchNotificationJobDisposition::Complete("inserted"),
        R::AlreadyExists => SearchFilterMatchNotificationJobDisposition::Complete("duplicate"),
        R::SuppressedByQuota => {
            SearchFilterMatchNotificationJobDisposition::Complete("suppressed_by_quota")
        }
        // User deletion is a terminal recipient suppression, not missing historical business truth.
        R::SuppressedForMissingUser => {
            SearchFilterMatchNotificationJobDisposition::Complete("missing_user")
        }
        R::SuppressedForWithdrawnProductListing => {
            SearchFilterMatchNotificationJobDisposition::Complete("withdrawn")
        }
        R::SuppressedForStaleMatch => {
            SearchFilterMatchNotificationJobDisposition::Complete("stale_match")
        }
        R::SuppressedForMissingMatch => {
            SearchFilterMatchNotificationJobDisposition::Retry("missing_match")
        }
        R::SuppressedForMissingProductListing => {
            SearchFilterMatchNotificationJobDisposition::Retry("missing_product")
        }
    }
}
fn command_from_job(
    job: DomainJob,
) -> Result<GenerateSearchFilterMatchNotificationCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterMatchCreated(change) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(GenerateSearchFilterMatchNotificationCommand {
        user_id: change.user_id,
        search_filter_id: change.user_search_filter_id,
        product_listing_id: change.product_listing_id,
        origin_event_id: change.origin_event_id,
    })
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
