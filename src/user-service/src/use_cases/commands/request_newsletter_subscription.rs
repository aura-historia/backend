use crate::ports::{
    NewNewsletterConfirmationChallenge, NewsletterConfirmationChallenges,
    NewsletterConfirmationChallengesFactory, NewsletterConfirmationClock,
    NewsletterConfirmationEmail, NewsletterConfirmationEmailSendOutcome,
    NewsletterConfirmationEmailSender, NewsletterConfirmationIssueOutcome,
    NewsletterConfirmationSendStatus, NewsletterConfirmationTokenGenerator, NewsletterProfile,
    NewsletterProfileReader,
};
use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use localization::Language;
use money::Currency;
use serde_email::Email;
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::newsletter_confirmation_id::NewsletterConfirmationId;

pub struct RequestNewsletterSubscriptionCommand {
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
}

#[derive(Debug, thiserror::Error)]
pub enum RequestNewsletterSubscriptionError {
    #[error("newsletter confirmation service temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("newsletter confirmation state is invalid")]
    InvalidPersistedState,
    #[error("newsletter confirmation email was definitely rejected")]
    EmailRejected,
    #[error("newsletter confirmation email acceptance is unknown")]
    EmailAcceptanceUnknown,
    #[error("newsletter confirmation token generation failed")]
    TokenGenerationFailed,
}

#[async_trait::async_trait]
pub trait RequestNewsletterSubscriptionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RequestNewsletterSubscriptionCommand,
    ) -> Result<(), RequestNewsletterSubscriptionError>;
}

pub struct RequestNewsletterSubscriptionHandler<U, C, R, S, G, K> {
    unit_of_work: U,
    challenges: C,
    profile_reader: R,
    sender: S,
    token_generator: G,
    clock: K,
}

impl<U, C, R, S, G, K> RequestNewsletterSubscriptionHandler<U, C, R, S, G, K> {
    pub fn new(
        unit_of_work: U,
        challenges: C,
        profile_reader: R,
        sender: S,
        token_generator: G,
        clock: K,
    ) -> Self {
        Self {
            unit_of_work,
            challenges,
            profile_reader,
            sender,
            token_generator,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, R, S, G, K> RequestNewsletterSubscriptionUseCase
    for RequestNewsletterSubscriptionHandler<U, C, R, S, G, K>
where
    U: UnitOfWork,
    C: NewsletterConfirmationChallengesFactory<U::Tx>,
    R: NewsletterProfileReader,
    S: NewsletterConfirmationEmailSender,
    G: NewsletterConfirmationTokenGenerator,
    K: NewsletterConfirmationClock,
{
    #[tracing::instrument(
        name = "request_newsletter_subscription",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RequestNewsletterSubscriptionCommand,
    ) -> Result<(), RequestNewsletterSubscriptionError> {
        let requester = match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(id),
            Principal::Anonymous | Principal::Service(_) | Principal::System => None,
        };
        if let Some(requester) = requester {
            tracing::Span::current().record("actor_id", tracing::field::display(requester));
        }

        // Authentication only supplies optional profile fallback and request metadata.
        // It never proves ownership of the requested target mailbox or skips DOI.
        let fallback = match requester {
            Some(user_id) => self
                .profile_reader
                .find_by_user_id(user_id)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let profile = NewsletterProfile {
            first_name: command
                .first_name
                .or_else(|| fallback.as_ref().and_then(|value| value.first_name.clone())),
            last_name: command
                .last_name
                .or_else(|| fallback.as_ref().and_then(|value| value.last_name.clone())),
            language: command
                .language
                .or_else(|| fallback.as_ref().and_then(|value| value.language)),
            currency: command
                .currency
                .or_else(|| fallback.as_ref().and_then(|value| value.currency)),
        };
        let now = self.clock.now_utc();
        let token = self
            .token_generator
            .generate()
            .map_err(|_| RequestNewsletterSubscriptionError::TokenGenerationFailed)?;
        let challenge_id = NewsletterConfirmationId::new();
        let email = command.email;
        let draft = NewNewsletterConfirmationChallenge {
            id: challenge_id,
            token_digest: token.digest(),
            email: email.clone(),
            requested_by_user_id: requester,
            profile: profile.clone(),
            now,
        };

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| RequestNewsletterSubscriptionError::TemporarilyUnavailable)?;
        let issue = self
            .challenges
            .in_transaction(&mut tx)
            .create_if_allowed(draft)
            .await
            .map_err(map_request_challenge_error)?;
        tx.commit()
            .await
            .map_err(|_| RequestNewsletterSubscriptionError::TemporarilyUnavailable)?;

        // A suppression is deliberately indistinguishable from a newly accepted
        // request to callers. The DB transaction has already committed before mail I/O.
        if issue == NewsletterConfirmationIssueOutcome::Suppressed {
            return Ok(());
        }

        let outcome = self
            .sender
            .send(NewsletterConfirmationEmail {
                confirmation_id: challenge_id,
                recipient: email,
                token,
                profile,
            })
            .await;
        let send_status = match outcome {
            NewsletterConfirmationEmailSendOutcome::Accepted => {
                NewsletterConfirmationSendStatus::Accepted
            }
            NewsletterConfirmationEmailSendOutcome::DefinitelyRejected => {
                NewsletterConfirmationSendStatus::DefinitelyRejected
            }
            NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown => {
                NewsletterConfirmationSendStatus::AcceptanceUnknown
            }
        };
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| RequestNewsletterSubscriptionError::TemporarilyUnavailable)?;
        self.challenges
            .in_transaction(&mut tx)
            .record_send_outcome(challenge_id, send_status, self.clock.now_utc())
            .await
            .map_err(map_request_challenge_error)?;
        tx.commit()
            .await
            .map_err(|_| RequestNewsletterSubscriptionError::TemporarilyUnavailable)?;

        if outcome == NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
            Err(RequestNewsletterSubscriptionError::EmailRejected)
        } else if outcome == NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown {
            Err(RequestNewsletterSubscriptionError::EmailAcceptanceUnknown)
        } else {
            Ok(())
        }
    }
}

fn map_request_challenge_error(
    error: crate::ports::NewsletterConfirmationChallengeError,
) -> RequestNewsletterSubscriptionError {
    match error {
        crate::ports::NewsletterConfirmationChallengeError::InvalidPersistedState => {
            RequestNewsletterSubscriptionError::InvalidPersistedState
        }
        crate::ports::NewsletterConfirmationChallengeError::InvalidInput
        | crate::ports::NewsletterConfirmationChallengeError::TemporarilyUnavailable { .. } => {
            RequestNewsletterSubscriptionError::TemporarilyUnavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        NewsletterConfirmationChallenge, NewsletterConfirmationChallengeError,
        NewsletterConfirmationChallenges, NewsletterConfirmationIssueOutcome,
        NewsletterConfirmationSendStatus, NewsletterConfirmationTokenGenerationError,
        NewsletterConfirmationTokenGenerator, NewsletterProfileReadError,
    };
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use user_core::newsletter_confirmation::{
        NewsletterConfirmationTokenDigest, RawNewsletterConfirmationToken,
    };
    use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
    use user_core::user_id::UserId;

    #[derive(Clone, Copy)]
    struct FixedClock;

    impl NewsletterConfirmationClock for FixedClock {
        fn now_utc(&self) -> OffsetDateTime {
            OffsetDateTime::UNIX_EPOCH
        }
    }

    #[derive(Clone)]
    struct FixedToken(RawNewsletterConfirmationToken);

    impl NewsletterConfirmationTokenGenerator for FixedToken {
        fn generate(
            &self,
        ) -> Result<RawNewsletterConfirmationToken, NewsletterConfirmationTokenGenerationError>
        {
            Ok(self.0.clone())
        }
    }

    struct State {
        begins: usize,
        commits: usize,
        issued: NewsletterConfirmationIssueOutcome,
        send_outcome: NewsletterConfirmationEmailSendOutcome,
        sender_called: usize,
        sender_after_commit: bool,
        token_received: bool,
        token_debug_redacted: bool,
        target: Option<Email>,
        requester: Option<UserId>,
        digest: Option<NewsletterConfirmationTokenDigest>,
        send_status: Option<NewsletterConfirmationSendStatus>,
        issue_calls: usize,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                begins: 0,
                commits: 0,
                issued: NewsletterConfirmationIssueOutcome::Suppressed,
                send_outcome: NewsletterConfirmationEmailSendOutcome::Accepted,
                sender_called: 0,
                sender_after_commit: false,
                token_received: false,
                token_debug_redacted: false,
                target: None,
                requester: None,
                digest: None,
                send_status: None,
                issue_calls: 0,
            }
        }
    }

    type Shared = Arc<Mutex<State>>;

    fn lock(state: &Shared) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct FakeTx(Shared);

    #[async_trait::async_trait]
    impl application::transaction::Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    struct FakeUow(Shared);

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUow {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTx(self.0.clone()))
        }
    }

    struct FakeChallenges(Shared);

    struct FakeChallengePort(Shared);

    impl NewsletterConfirmationChallengesFactory<FakeTx> for FakeChallenges {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut FakeTx,
        ) -> impl NewsletterConfirmationChallenges + 'tx {
            FakeChallengePort(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl NewsletterConfirmationChallenges for FakeChallengePort {
        async fn create_if_allowed(
            &mut self,
            challenge: NewNewsletterConfirmationChallenge,
        ) -> Result<NewsletterConfirmationIssueOutcome, NewsletterConfirmationChallengeError>
        {
            let mut state = lock(&self.0);
            state.issue_calls += 1;
            state.target = Some(challenge.email);
            state.requester = challenge.requested_by_user_id;
            state.digest = Some(challenge.token_digest);
            Ok(state.issued)
        }

        async fn record_send_outcome(
            &mut self,
            _: NewsletterConfirmationId,
            outcome: NewsletterConfirmationSendStatus,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            lock(&self.0).send_status = Some(outcome);
            Ok(())
        }

        async fn find_by_token_digest(
            &mut self,
            _: NewsletterConfirmationTokenDigest,
        ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError>
        {
            unreachable!()
        }

        async fn lock_for_confirmation(
            &mut self,
            _: NewsletterConfirmationId,
            _: NewsletterConfirmationTokenDigest,
        ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError>
        {
            unreachable!()
        }

        async fn invalidate(
            &mut self,
            _: NewsletterConfirmationId,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn complete_confirmation(
            &mut self,
            _: NewsletterConfirmationId,
            _: Option<UserId>,
            _: user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn invalidate_pending_for_email(
            &mut self,
            _: &Email,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn cleanup_expired(
            &mut self,
            _: OffsetDateTime,
            _: u16,
        ) -> Result<u64, NewsletterConfirmationChallengeError> {
            unreachable!()
        }
    }

    struct FakeProfileReader;

    #[async_trait::async_trait]
    impl NewsletterProfileReader for FakeProfileReader {
        async fn find_by_user_id(
            &self,
            _: UserId,
        ) -> Result<Option<NewsletterProfile>, NewsletterProfileReadError> {
            Ok(Some(NewsletterProfile::default()))
        }
    }

    struct FakeSender(Shared);

    #[async_trait::async_trait]
    impl NewsletterConfirmationEmailSender for FakeSender {
        async fn send(
            &self,
            email: NewsletterConfirmationEmail,
        ) -> NewsletterConfirmationEmailSendOutcome {
            let mut state = lock(&self.0);
            state.sender_called += 1;
            state.sender_after_commit = state.commits == 1;
            state.token_received =
                email.token.as_str() == "WlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlo";
            state.token_debug_redacted = format!("{:?}", email.token).contains("REDACTED");
            state.send_outcome
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command() -> RequestNewsletterSubscriptionCommand {
        RequestNewsletterSubscriptionCommand {
            email: Email::try_from("requested-target@example.test").unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        }
    }

    fn handler(
        state: &Shared,
    ) -> RequestNewsletterSubscriptionHandler<
        FakeUow,
        FakeChallenges,
        FakeProfileReader,
        FakeSender,
        FixedToken,
        FixedClock,
    > {
        RequestNewsletterSubscriptionHandler::new(
            FakeUow(state.clone()),
            FakeChallenges(state.clone()),
            FakeProfileReader,
            FakeSender(state.clone()),
            FixedToken(RawNewsletterConfirmationToken::from_entropy([0x5a; 32])),
            FixedClock,
        )
    }

    #[tokio::test]
    async fn anonymous_and_authenticated_alternate_mailbox_requests_both_send_confirmation() {
        let authenticated_id = UserId::new();
        for (principal, requester) in [
            (Principal::Anonymous, None),
            (Principal::User(authenticated_id), Some(authenticated_id)),
        ] {
            let state = Arc::new(Mutex::new(State {
                issued: NewsletterConfirmationIssueOutcome::Issued,
                send_outcome: NewsletterConfirmationEmailSendOutcome::Accepted,
                ..State::default()
            }));
            handler(&state)
                .execute(&context(principal), command())
                .await
                .unwrap();
            let state = lock(&state);
            assert_eq!(1, state.sender_called);
            assert!(state.sender_after_commit);
            assert!(state.token_received);
            assert!(state.token_debug_redacted);
            assert_eq!(requester, state.requester);
            assert_eq!(
                Some(Email::try_from("requested-target@example.test").unwrap()),
                state.target
            );
            assert!(state.digest.is_some());
        }
    }

    #[tokio::test]
    async fn suppression_has_the_same_accepted_result_and_does_not_send() {
        let state = Arc::new(Mutex::new(State {
            issued: NewsletterConfirmationIssueOutcome::Suppressed,
            ..State::default()
        }));
        assert!(
            handler(&state)
                .execute(&context(Principal::Anonymous), command())
                .await
                .is_ok()
        );
        let state = lock(&state);
        assert_eq!(1, state.issue_calls);
        assert_eq!(0, state.sender_called);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn send_outcomes_record_safe_status_and_categorize_non_successes() {
        for (outcome, status, expected_error) in [
            (
                NewsletterConfirmationEmailSendOutcome::Accepted,
                NewsletterConfirmationSendStatus::Accepted,
                None,
            ),
            (
                NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown,
                NewsletterConfirmationSendStatus::AcceptanceUnknown,
                Some("unknown"),
            ),
            (
                NewsletterConfirmationEmailSendOutcome::DefinitelyRejected,
                NewsletterConfirmationSendStatus::DefinitelyRejected,
                Some("rejected"),
            ),
        ] {
            let state = Arc::new(Mutex::new(State {
                issued: NewsletterConfirmationIssueOutcome::Issued,
                send_outcome: outcome,
                ..State::default()
            }));
            let result = handler(&state)
                .execute(&context(Principal::Anonymous), command())
                .await;
            if let Err(error) = &result {
                let message = format!("{error:?}");
                assert!(!message.contains("requested-target@example.test"));
                assert!(!message.contains("WlpaWlpa"));
            }
            match (result, expected_error) {
                (Ok(()), None) => {}
                (
                    Err(RequestNewsletterSubscriptionError::EmailAcceptanceUnknown),
                    Some("unknown"),
                )
                | (Err(RequestNewsletterSubscriptionError::EmailRejected), Some("rejected")) => {}
                (result, expected) => panic!("unexpected request result: {result:?}, {expected:?}"),
            }
            let state = lock(&state);
            assert_eq!(Some(status), state.send_status);
        }
    }
}
