use crate::ports::{
    NewsletterConfirmationChallenges, NewsletterConfirmationChallengesFactory,
    NewsletterConfirmationClock,
};
use application::transaction::{Transaction, UnitOfWork};

const MAX_CLEANUP_BATCH_SIZE: u16 = 1_000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CleanupNewsletterConfirmationChallengesError {
    #[error("cleanup batch size must be between 1 and 1000")]
    InvalidBatchSize,
    #[error("newsletter confirmation cleanup temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("newsletter confirmation state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait CleanupNewsletterConfirmationChallengesUseCase: Send + Sync {
    async fn execute(
        &self,
        batch_size: u16,
    ) -> Result<u64, CleanupNewsletterConfirmationChallengesError>;
}

pub struct CleanupNewsletterConfirmationChallengesHandler<U, C, K> {
    unit_of_work: U,
    challenges: C,
    clock: K,
}

impl<U, C, K> CleanupNewsletterConfirmationChallengesHandler<U, C, K> {
    pub fn new(unit_of_work: U, challenges: C, clock: K) -> Self {
        Self {
            unit_of_work,
            challenges,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, K> CleanupNewsletterConfirmationChallengesUseCase
    for CleanupNewsletterConfirmationChallengesHandler<U, C, K>
where
    U: UnitOfWork,
    C: NewsletterConfirmationChallengesFactory<U::Tx>,
    K: NewsletterConfirmationClock,
{
    async fn execute(
        &self,
        batch_size: u16,
    ) -> Result<u64, CleanupNewsletterConfirmationChallengesError> {
        if !(1..=MAX_CLEANUP_BATCH_SIZE).contains(&batch_size) {
            return Err(CleanupNewsletterConfirmationChallengesError::InvalidBatchSize);
        }
        let mut tx =
            self.unit_of_work.begin().await.map_err(|_| {
                CleanupNewsletterConfirmationChallengesError::TemporarilyUnavailable
            })?;
        let removed = self
            .challenges
            .in_transaction(&mut tx)
            .cleanup_expired(self.clock.now_utc(), batch_size)
            .await
            .map_err(|error| match error {
                crate::ports::NewsletterConfirmationChallengeError::InvalidPersistedState => {
                    CleanupNewsletterConfirmationChallengesError::InvalidPersistedState
                }
                crate::ports::NewsletterConfirmationChallengeError::InvalidInput
                | crate::ports::NewsletterConfirmationChallengeError::TemporarilyUnavailable {
                    ..
                } => CleanupNewsletterConfirmationChallengesError::TemporarilyUnavailable,
            })?;
        tx.commit()
            .await
            .map_err(|_| CleanupNewsletterConfirmationChallengesError::TemporarilyUnavailable)?;
        Ok(removed)
    }
}
