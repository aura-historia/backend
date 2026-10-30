use super::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationError,
    GenerateSearchFilterMatchNotificationUseCase,
};
use application::error::BoxError;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use std::sync::Arc;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
#[error("saved-search notification recovery read failed")]
pub struct SavedSearchNotificationRecoveryError {
    #[source]
    pub source: BoxError,
}

/// A deferred selection whose notification generation failed; the error keeps its cause for the
/// boundary that records it.
#[derive(Debug)]
pub struct SavedSearchNotificationRecoveryFailure {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub error: GenerateSearchFilterMatchNotificationError,
}

#[derive(Debug, Default)]
pub struct SavedSearchNotificationRecoveryReport {
    pub recovered: usize,
    pub failures: Vec<SavedSearchNotificationRecoveryFailure>,
}

#[async_trait::async_trait]
pub trait PendingSavedSearchNotificationReader: Send + Sync {
    async fn defer(
        &self,
        command: &GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<(), SavedSearchNotificationRecoveryError>;
    async fn find_pending(
        &self,
        limit: usize,
    ) -> Result<
        Vec<GenerateSearchFilterMatchNotificationCommand>,
        SavedSearchNotificationRecoveryError,
    >;
}

#[async_trait::async_trait]
pub trait RecoverSavedSearchNotificationsUseCase: Send + Sync {
    /// Generates up to `limit` pending notifications. A failed selection is deferred and
    /// reported while the others continue.
    async fn execute(
        &self,
        limit: usize,
    ) -> Result<SavedSearchNotificationRecoveryReport, SavedSearchNotificationRecoveryError>;
}

pub struct RecoverSavedSearchNotificationsHandler<R> {
    pending: R,
    generator: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
}
impl<R: PendingSavedSearchNotificationReader> RecoverSavedSearchNotificationsHandler<R> {
    pub fn new(
        pending: R,
        generator: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
    ) -> Self {
        Self { pending, generator }
    }
}

#[async_trait::async_trait]
impl<R: PendingSavedSearchNotificationReader> RecoverSavedSearchNotificationsUseCase
    for RecoverSavedSearchNotificationsHandler<R>
{
    async fn execute(
        &self,
        limit: usize,
    ) -> Result<SavedSearchNotificationRecoveryReport, SavedSearchNotificationRecoveryError> {
        let commands = self.pending.find_pending(limit).await?;
        let mut report = SavedSearchNotificationRecoveryReport::default();
        for command in commands {
            let user_id = command.user_id;
            let filter_id = command.search_filter_id;
            match self.generator.execute(command.clone()).await {
                Ok(outcome) => {
                    report.recovered += 1;
                    tracing::info!(%user_id, %filter_id, ?outcome, "saved_search.notification.recovered")
                }
                Err(error) => {
                    self.pending.defer(&command).await?;
                    report
                        .failures
                        .push(SavedSearchNotificationRecoveryFailure {
                            user_id,
                            search_filter_id: filter_id,
                            error,
                        });
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::GenerateSearchFilterMatchNotificationResult;
    use std::sync::Mutex;

    struct Pending {
        commands: Vec<GenerateSearchFilterMatchNotificationCommand>,
        deferred: Arc<Mutex<Vec<UserSearchFilterId>>>,
    }
    #[async_trait::async_trait]
    impl PendingSavedSearchNotificationReader for Pending {
        async fn find_pending(
            &self,
            limit: usize,
        ) -> Result<
            Vec<GenerateSearchFilterMatchNotificationCommand>,
            SavedSearchNotificationRecoveryError,
        > {
            Ok(self.commands.iter().take(limit).cloned().collect())
        }
        async fn defer(
            &self,
            command: &GenerateSearchFilterMatchNotificationCommand,
        ) -> Result<(), SavedSearchNotificationRecoveryError> {
            self.deferred.lock().unwrap().push(command.search_filter_id);
            Ok(())
        }
    }
    struct Generator {
        failed_filter: UserSearchFilterId,
        executed: Arc<Mutex<Vec<UserSearchFilterId>>>,
    }
    #[async_trait::async_trait]
    impl GenerateSearchFilterMatchNotificationUseCase for Generator {
        async fn execute(
            &self,
            command: GenerateSearchFilterMatchNotificationCommand,
        ) -> Result<
            GenerateSearchFilterMatchNotificationResult,
            GenerateSearchFilterMatchNotificationError,
        > {
            self.executed.lock().unwrap().push(command.search_filter_id);
            if command.search_filter_id == self.failed_filter {
                Err(
                    GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed {
                        source: application::error::box_error(std::io::Error::other(
                            "dependency unavailable",
                        )),
                    },
                )
            } else {
                Ok(GenerateSearchFilterMatchNotificationResult::Created)
            }
        }
    }
    #[tokio::test]
    async fn failed_selection_is_deferred_while_other_users_continue_recovery() {
        let command = || GenerateSearchFilterMatchNotificationCommand {
            user_id: user_core::user_id::UserId::new(),
            search_filter_id: UserSearchFilterId::new(),
            product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(),
            origin_event_id: domain_primitives::event_id::EventId::new(),
        };
        let failed = command();
        let successful = command();
        let deferred = Arc::new(Mutex::new(Vec::new()));
        let executed = Arc::new(Mutex::new(Vec::new()));
        let handler = RecoverSavedSearchNotificationsHandler::new(
            Pending {
                commands: vec![failed.clone(), successful.clone()],
                deferred: deferred.clone(),
            },
            Arc::new(Generator {
                failed_filter: failed.search_filter_id,
                executed: executed.clone(),
            }),
        );
        let report = handler.execute(2).await.unwrap();
        assert_eq!(1, report.recovered);
        assert!(matches!(
            report.failures.as_slice(),
            [failure] if failure.search_filter_id == failed.search_filter_id
                && matches!(
                    failure.error,
                    GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed { .. }
                )
        ));
        assert_eq!(vec![failed.search_filter_id], *deferred.lock().unwrap());
        assert_eq!(
            vec![failed.search_filter_id, successful.search_filter_id],
            *executed.lock().unwrap()
        );
    }
}
