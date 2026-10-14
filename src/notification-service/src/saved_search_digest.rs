use application::{
    error::{BoxError, box_error},
    transaction::{Transaction, UnitOfWork},
};
use notification_core::notification::NotificationContent;
use time::OffsetDateTime;
use user_core::user_id::UserId;

/// Versioned, immutable membership. Rendering deduplicates listing cards while
/// retaining every selected per-filter match for accounting and search labels.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedSearchEmailDigest {
    pub digest_id: notification_core::saved_search_digest_id::SavedSearchDigestId,
    pub window_end: OffsetDateTime,
    pub members: Vec<NotificationContent>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SavedSearchDigestMaintenanceReport {
    pub groups_finalized: usize,
    pub digests_created: usize,
    pub matches_enrolled: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("saved-search digest maintenance failed")]
pub struct SavedSearchDigestError {
    #[source]
    pub source: BoxError,
}

#[async_trait::async_trait]
pub trait SavedSearchDigestStore: Send {
    /// Serialize with membership changes and other finalizers, then finalize this
    /// user/run only when ready under the normal completion or expired-run rules.
    /// The readiness check must observe completions committed while waiting for
    /// the group lock, so concurrent completions cannot both miss finalization.
    async fn finalize_for_user(
        &mut self,
        user_id: UserId,
        window_end: OffsetDateTime,
    ) -> Result<Option<SavedSearchDigestMaintenanceReport>, SavedSearchDigestError>;

    /// Expire interrupted runs and finalize one ready user group under a lock.
    /// Membership and the existing delivery outbox commit in the caller's transaction.
    async fn finalize_next(
        &mut self,
    ) -> Result<Option<SavedSearchDigestMaintenanceReport>, SavedSearchDigestError>;
}
pub trait SavedSearchDigestStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl SavedSearchDigestStore + 'a;
}

pub struct MaintainSavedSearchDigestsHandler<U, S> {
    unit_of_work: U,
    store: S,
}
impl<U, S> MaintainSavedSearchDigestsHandler<U, S> {
    pub fn new(unit_of_work: U, store: S) -> Self {
        Self {
            unit_of_work,
            store,
        }
    }
}
impl<U: UnitOfWork, S: SavedSearchDigestStoreFactory<U::Tx>>
    MaintainSavedSearchDigestsHandler<U, S>
{
    pub async fn execute(
        &self,
        limit: usize,
    ) -> Result<SavedSearchDigestMaintenanceReport, SavedSearchDigestError> {
        let mut report = SavedSearchDigestMaintenanceReport::default();
        for _ in 0..limit {
            let mut tx =
                self.unit_of_work
                    .begin()
                    .await
                    .map_err(|source| SavedSearchDigestError {
                        source: box_error(source),
                    })?;
            let result = self.store.in_transaction(&mut tx).finalize_next().await?;
            tx.commit().await.map_err(|source| SavedSearchDigestError {
                source: box_error(source),
            })?;
            let Some(result) = result else { break };
            report.groups_finalized += result.groups_finalized;
            report.digests_created += result.digests_created;
            report.matches_enrolled += result.matches_enrolled;
        }
        tracing::info!(
            groups_finalized = report.groups_finalized,
            digests_created = report.digests_created,
            eligible_matches = report.matches_enrolled,
            "saved_search.digest.maintenance"
        );
        Ok(report)
    }
}
