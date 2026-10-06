use crate::ports::{
    ConsentUser, MarketingConsentIntents, MarketingConsentIntentsFactory,
    NewsletterConfirmationChallengeError, NewsletterConfirmationChallenges,
    NewsletterConfirmationChallengesFactory, NewsletterConfirmationClock,
};
use crate::use_cases::commands::coordinate_marketing_consent::{
    CoordinateMarketingConsentError, MarketingConsentCoordinator,
};
use application::transaction::{Transaction, UnitOfWork};
use serde_email::Email;
use user_core::newsletter_confirmation::RawNewsletterConfirmationToken;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfirmNewsletterSubscriptionError {
    #[error("invalid newsletter confirmation")]
    InvalidConfirmation,
    #[error("newsletter confirmation service temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("newsletter confirmation state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait ConfirmNewsletterSubscriptionUseCase: Send + Sync {
    async fn execute(&self, token: &str) -> Result<(), ConfirmNewsletterSubscriptionError>;
}

pub struct ConfirmNewsletterSubscriptionHandler<U, C, I, K> {
    unit_of_work: U,
    challenges: C,
    intents: I,
    clock: K,
}

impl<U, C, I, K> ConfirmNewsletterSubscriptionHandler<U, C, I, K> {
    pub fn new(unit_of_work: U, challenges: C, intents: I, clock: K) -> Self {
        Self {
            unit_of_work,
            challenges,
            intents,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, I, K> ConfirmNewsletterSubscriptionUseCase
    for ConfirmNewsletterSubscriptionHandler<U, C, I, K>
where
    U: UnitOfWork,
    C: NewsletterConfirmationChallengesFactory<U::Tx>,
    I: MarketingConsentIntentsFactory<U::Tx>,
    K: NewsletterConfirmationClock,
{
    async fn execute(&self, input: &str) -> Result<(), ConfirmNewsletterSubscriptionError> {
        let token = RawNewsletterConfirmationToken::try_from(input)
            .map_err(|_| ConfirmNewsletterSubscriptionError::InvalidConfirmation)?;
        let digest = token.digest();
        let now = self.clock.now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)?;

        let candidate = self
            .challenges
            .in_transaction(&mut tx)
            .find_by_token_digest(digest)
            .await
            .map_err(map_challenge_error)?;
        let Some(candidate) = candidate else {
            return Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation);
        };
        let confirmation_id_text = candidate.id.to_string();

        // Match C02's source-key -> recipient-key -> row lock order. This serializes
        // duplicate proof use before the shared mailbox lock is acquired.
        MarketingConsentCoordinator::new(&mut tx, &self.intents)
            .lock_accepted_double_opt_in_source(&confirmation_id_text)
            .await
            .map_err(map_consent_error)?;

        let challenge = self
            .challenges
            .in_transaction(&mut tx)
            .lock_for_confirmation(candidate.id, digest)
            .await
            .map_err(map_challenge_error)?;
        let Some(challenge) = challenge else {
            return Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation);
        };

        // A confirmed proof remains an idempotent success after later withdrawal.
        if challenge.confirmed_at.is_some() {
            if challenge.resulting_intent_id.is_none() {
                return Err(ConfirmNewsletterSubscriptionError::InvalidPersistedState);
            }
            tx.commit()
                .await
                .map_err(|_| ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)?;
            return Ok(());
        }
        if challenge.invalidated_at.is_some() {
            return Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation);
        }
        if now >= challenge.expires_at {
            self.challenges
                .in_transaction(&mut tx)
                .invalidate(challenge.id, now)
                .await
                .map_err(map_challenge_error)?;
            tx.commit()
                .await
                .map_err(|_| ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)?;
            return Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation);
        }

        // Only a binding discovered from this exact target mailbox is ownership
        // evidence. The requester ID is metadata and is never consulted here.
        let bound_user_id = match challenge.bound_user_id {
            Some(bound_user_id) => {
                match find_user_by_id(&self.intents, &mut tx, bound_user_id).await? {
                    Some(user) if exact_email(&user.email, &challenge.email) => Some(bound_user_id),
                    Some(_) => {
                        self.challenges
                            .in_transaction(&mut tx)
                            .invalidate(challenge.id, now)
                            .await
                            .map_err(map_challenge_error)?;
                        tx.commit().await.map_err(|_| {
                            ConfirmNewsletterSubscriptionError::TemporarilyUnavailable
                        })?;
                        return Err(ConfirmNewsletterSubscriptionError::InvalidPersistedState);
                    }
                    None => {
                        self.challenges
                            .in_transaction(&mut tx)
                            .invalidate(challenge.id, now)
                            .await
                            .map_err(map_challenge_error)?;
                        tx.commit().await.map_err(|_| {
                            ConfirmNewsletterSubscriptionError::TemporarilyUnavailable
                        })?;
                        return Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation);
                    }
                }
            }
            None => find_user_by_email(&self.intents, &mut tx, &challenge.email)
                .await?
                .map(|user| user.user_id),
        };

        let intent_id = MarketingConsentCoordinator::new(&mut tx, &self.intents)
            .accepted_double_opt_in_with_profile(
                confirmation_id_text,
                challenge.email.clone(),
                Some(challenge.profile.clone()),
                now,
            )
            .await
            .map_err(map_consent_error)?;
        self.challenges
            .in_transaction(&mut tx)
            .complete_confirmation(challenge.id, bound_user_id, intent_id, now)
            .await
            .map_err(map_challenge_error)?;

        tx.commit()
            .await
            .map_err(|_| ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)
    }
}

async fn find_user_by_id<Tx, I>(
    intents: &I,
    tx: &mut Tx,
    user_id: UserId,
) -> Result<Option<ConsentUser>, ConfirmNewsletterSubscriptionError>
where
    Tx: Transaction,
    I: MarketingConsentIntentsFactory<Tx>,
{
    intents
        .in_transaction(tx)
        .find_user_by_id(user_id)
        .await
        .map_err(|_| ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)
}

async fn find_user_by_email<Tx, I>(
    intents: &I,
    tx: &mut Tx,
    email: &Email,
) -> Result<Option<ConsentUser>, ConfirmNewsletterSubscriptionError>
where
    Tx: Transaction,
    I: MarketingConsentIntentsFactory<Tx>,
{
    intents
        .in_transaction(tx)
        .find_user_by_email(email)
        .await
        .map_err(|_| ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)
}

fn exact_email(actual: &Email, expected: &Email) -> bool {
    <Email as AsRef<str>>::as_ref(actual) == <Email as AsRef<str>>::as_ref(expected)
}

fn map_challenge_error(
    error: NewsletterConfirmationChallengeError,
) -> ConfirmNewsletterSubscriptionError {
    match error {
        NewsletterConfirmationChallengeError::InvalidPersistedState => {
            ConfirmNewsletterSubscriptionError::InvalidPersistedState
        }
        NewsletterConfirmationChallengeError::InvalidInput
        | NewsletterConfirmationChallengeError::TemporarilyUnavailable { .. } => {
            ConfirmNewsletterSubscriptionError::TemporarilyUnavailable
        }
    }
}

fn map_consent_error(error: CoordinateMarketingConsentError) -> ConfirmNewsletterSubscriptionError {
    match error {
        CoordinateMarketingConsentError::InvalidPersistedState
        | CoordinateMarketingConsentError::EmailMismatch
        | CoordinateMarketingConsentError::SourceKeyConflict
        | CoordinateMarketingConsentError::InvalidIdentity => {
            ConfirmNewsletterSubscriptionError::InvalidPersistedState
        }
        CoordinateMarketingConsentError::TemporarilyUnavailable
        | CoordinateMarketingConsentError::ConcurrencyConflict
        | CoordinateMarketingConsentError::UserNotFound
        | CoordinateMarketingConsentError::BeginTransactionFailed
        | CoordinateMarketingConsentError::CommitTransactionFailed
        | CoordinateMarketingConsentError::Forbidden => {
            ConfirmNewsletterSubscriptionError::TemporarilyUnavailable
        }
    }
}
