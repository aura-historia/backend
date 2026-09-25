#[cfg(test)]
use crate::cdc::{CdcOperation, DomainJob, DomainJobPayload};
use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
#[cfg(test)]
use search_filter_projection_lambda::command_from_job;
use search_filter_projection_lambda::execute_job;
pub use search_filter_projection_lambda::{
    SearchFilterProjectionJobDisposition, process_search_filter_projection_job,
};
use search_filter_service::use_cases::ProjectSearchFilterChangeUseCase;
#[cfg(test)]
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeCommand, SearchFilterProjectionOperation,
};
use std::sync::Arc;

pub async fn consume_search_filter_projection_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    handler: Arc<dyn ProjectSearchFilterChangeUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterProjection, move |job| {
            let handler = Arc::clone(&handler);
            async move { polling_outcome(execute_job(handler.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: SearchFilterProjectionJobDisposition) -> JobOutcome {
    match disposition {
        SearchFilterProjectionJobDisposition::Complete(category) => JobOutcome::Complete(category),
        SearchFilterProjectionJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        SearchFilterProjectionJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, SearchFilterChangedJob, WorkerQueue};
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use user_core::user_id::UserId;
    use uuid::Uuid;

    #[test]
    fn should_map_insert_update_and_delete_without_inventing_projection_completion()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001))?;
        let id = UserSearchFilterId::try_from(Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0006,
        ))?;
        assert_eq!("usr_01j0000000e008000000000001", user_id.to_string());
        assert_eq!("sf_01j0000000e008000000000006", id.to_string());
        for (operation, expected) in [
            (
                CdcOperation::Insert,
                SearchFilterProjectionOperation::Upsert,
            ),
            (
                CdcOperation::Update,
                SearchFilterProjectionOperation::Upsert,
            ),
            (
                CdcOperation::Delete,
                SearchFilterProjectionOperation::Delete,
            ),
        ] {
            let command = command_from_job(DomainJob {
                target_queue: WorkerQueue::SearchFilterOpenSearch,
                idempotency_key: IdempotencyKey::new(format!("search-filter:{id}:3:{operation}")),
                ordering_key: OrderingKey::new(format!("search-filter:{id}")),
                payload: DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
                    user_id,
                    user_search_filter_id: id,
                    version: 3,
                    operation,
                }),
            });
            assert_eq!(
                Ok(ProjectSearchFilterChangeCommand {
                    search_filter_id: id,
                    source_version: 3,
                    operation: expected
                }),
                command
            );
        }
        Ok(())
    }
}
