use crate::ports::{
    NewsletterProfileReader, NewsletterSubscriptionWriteError, NewsletterSubscriptionWriter,
};
use common::error::boxed::BoxError;
use common::operation_context::{OperationContext, Principal};
use common::{currency::domain::Currency, language::domain::Language};
use serde_email::Email;
use user_core::{
    first_name::FirstName, last_name::LastName, newsletter_subscription::NewsletterSubscription,
};

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertNewsletterSubscriptionCommand {
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpsertNewsletterSubscriptionError {
    #[error("invalid newsletter subscription email")]
    InvalidEmail,
    #[error("newsletter subscription service temporarily unavailable")]
    NewsletterSubscriptionUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal newsletter subscription failure")]
    NewsletterSubscriptionInternal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UpsertNewsletterSubscriptionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertNewsletterSubscriptionCommand,
    ) -> Result<(), UpsertNewsletterSubscriptionError>;
}

pub struct UpsertNewsletterSubscriptionHandler<R, W> {
    profile_reader: R,
    subscription_writer: W,
}

impl<R, W> UpsertNewsletterSubscriptionHandler<R, W> {
    pub fn new(profile_reader: R, subscription_writer: W) -> Self {
        Self {
            profile_reader,
            subscription_writer,
        }
    }
}

#[async_trait::async_trait]
impl<R, W> UpsertNewsletterSubscriptionUseCase for UpsertNewsletterSubscriptionHandler<R, W>
where
    R: NewsletterProfileReader,
    W: NewsletterSubscriptionWriter,
{
    #[tracing::instrument(
        name = "upsert_newsletter_subscription",
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
        command: UpsertNewsletterSubscriptionCommand,
    ) -> Result<(), UpsertNewsletterSubscriptionError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let user_id = match &context.principal {
            Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
            Principal::Anonymous | Principal::Service(_) | Principal::System => None,
        };
        let profile = match user_id {
            Some(user_id) => match self.profile_reader.find_by_user_id(user_id).await {
                Ok(profile) => profile,
                Err(error) => {
                    let error_kind = match error {
                        crate::ports::NewsletterProfileReadError::TemporarilyUnavailable {
                            ..
                        } => "temporarily_unavailable",
                        crate::ports::NewsletterProfileReadError::InvalidReadModel { .. } => {
                            "invalid_read_model"
                        }
                        crate::ports::NewsletterProfileReadError::Internal { .. } => "internal",
                    };
                    tracing::debug!(
                        user_id = %user_id,
                        error_kind,
                        "newsletter profile fallback unavailable"
                    );
                    None
                }
            },
            None => None,
        };

        let subscription = NewsletterSubscription::new(
            command.email,
            command.first_name.or_else(|| {
                profile
                    .as_ref()
                    .and_then(|profile| profile.first_name.clone())
            }),
            command.last_name.or_else(|| {
                profile
                    .as_ref()
                    .and_then(|profile| profile.last_name.clone())
            }),
            command
                .language
                .or_else(|| profile.as_ref().and_then(|profile| profile.language)),
            command
                .currency
                .or_else(|| profile.as_ref().and_then(|profile| profile.currency)),
            user_id,
        );

        self.subscription_writer.upsert(&subscription).await?;

        Ok(())
    }
}

impl From<NewsletterSubscriptionWriteError> for UpsertNewsletterSubscriptionError {
    fn from(error: NewsletterSubscriptionWriteError) -> Self {
        match error {
            NewsletterSubscriptionWriteError::InvalidEmail => Self::InvalidEmail,
            NewsletterSubscriptionWriteError::TemporarilyUnavailable { source } => {
                Self::NewsletterSubscriptionUnavailable { source }
            }
            NewsletterSubscriptionWriteError::Internal { source } => {
                Self::NewsletterSubscriptionInternal { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        UpsertNewsletterSubscriptionCommand, UpsertNewsletterSubscriptionError,
        UpsertNewsletterSubscriptionHandler, UpsertNewsletterSubscriptionUseCase,
    };
    use crate::ports::{
        NewsletterProfile, NewsletterProfileReadError, NewsletterProfileReader,
        NewsletterSubscriptionWriteError, NewsletterSubscriptionWriter,
    };
    use common::error::boxed::{BoxError, box_error};
    use common::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use common::user_id::UserId;
    use serde_email::Email;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::newsletter_subscription::NewsletterSubscription;

    #[derive(Debug, Clone, Copy)]
    enum ProfileErrorKind {
        TemporarilyUnavailable,
        InvalidReadModel,
        Internal,
    }

    #[derive(Debug, Clone, Copy)]
    enum WriterErrorKind {
        InvalidEmail,
        TemporarilyUnavailable,
        Internal,
    }

    #[derive(Default)]
    struct ProfileReaderState {
        profile: Option<NewsletterProfile>,
        error: Option<ProfileErrorKind>,
        calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeNewsletterProfileReader {
        state: Arc<Mutex<ProfileReaderState>>,
    }

    #[derive(Default)]
    struct SubscriptionWriterState {
        subscription: Option<NewsletterSubscription>,
        error: Option<WriterErrorKind>,
        calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeNewsletterSubscriptionWriter {
        state: Arc<Mutex<SubscriptionWriterState>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn command() -> UpsertNewsletterSubscriptionCommand {
        UpsertNewsletterSubscriptionCommand {
            email: email("newsletter@example.com"),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        }
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("test failure"))
    }

    fn profile_error(kind: ProfileErrorKind) -> NewsletterProfileReadError {
        match kind {
            ProfileErrorKind::TemporarilyUnavailable => {
                NewsletterProfileReadError::TemporarilyUnavailable { source: boxed() }
            }
            ProfileErrorKind::InvalidReadModel => {
                NewsletterProfileReadError::InvalidReadModel { source: boxed() }
            }
            ProfileErrorKind::Internal => NewsletterProfileReadError::Internal { source: boxed() },
        }
    }

    fn writer_error(kind: WriterErrorKind) -> NewsletterSubscriptionWriteError {
        match kind {
            WriterErrorKind::InvalidEmail => NewsletterSubscriptionWriteError::InvalidEmail,
            WriterErrorKind::TemporarilyUnavailable => {
                NewsletterSubscriptionWriteError::TemporarilyUnavailable { source: boxed() }
            }
            WriterErrorKind::Internal => {
                NewsletterSubscriptionWriteError::Internal { source: boxed() }
            }
        }
    }

    #[async_trait::async_trait]
    impl NewsletterProfileReader for FakeNewsletterProfileReader {
        async fn find_by_user_id(
            &self,
            _user_id: UserId,
        ) -> Result<Option<NewsletterProfile>, NewsletterProfileReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(kind) => Err(profile_error(kind)),
                None => Ok(state.profile.clone()),
            }
        }
    }

    #[async_trait::async_trait]
    impl NewsletterSubscriptionWriter for FakeNewsletterSubscriptionWriter {
        async fn upsert(
            &self,
            subscription: &NewsletterSubscription,
        ) -> Result<(), NewsletterSubscriptionWriteError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(kind) => Err(writer_error(kind)),
                None => {
                    state.subscription = Some(subscription.clone());
                    Ok(())
                }
            }
        }
    }

    #[tokio::test]
    async fn should_not_read_profile_when_anonymous() {
        let reader = FakeNewsletterProfileReader::default();
        let writer = FakeNewsletterSubscriptionWriter::default();

        let result = UpsertNewsletterSubscriptionHandler::new(reader.clone(), writer.clone())
            .execute(&context(Principal::Anonymous), command())
            .await;

        assert!(result.is_ok());
        assert_eq!(0, lock(&reader.state).calls);
        let state = lock(&writer.state);
        assert_eq!(1, state.calls);
        assert_eq!(
            None,
            state
                .subscription
                .as_ref()
                .and_then(|value| value.user_id())
        );
    }

    #[tokio::test]
    async fn should_use_user_profile_as_fallback() {
        let user_id = UserId::new();
        let reader = FakeNewsletterProfileReader::default();
        lock(&reader.state).profile = Some(NewsletterProfile {
            first_name: Some("Ada".into()),
            last_name: Some("Lovelace".into()),
            language: Some(common::language::domain::Language::En),
            currency: Some(common::currency::domain::Currency::Eur),
        });
        let writer = FakeNewsletterSubscriptionWriter::default();

        let result = UpsertNewsletterSubscriptionHandler::new(reader.clone(), writer.clone())
            .execute(&context(Principal::User(user_id)), command())
            .await;

        assert!(result.is_ok());
        assert_eq!(1, lock(&reader.state).calls);
        let state = lock(&writer.state);
        let subscription = match state.subscription.as_ref() {
            Some(subscription) => subscription,
            None => panic!("expected subscription"),
        };
        assert_eq!(Some("Ada"), subscription.first_name().map(AsRef::as_ref));
        assert_eq!(
            Some("Lovelace"),
            subscription.last_name().map(AsRef::as_ref)
        );
        assert_eq!(
            Some(common::language::domain::Language::En),
            subscription.language()
        );
        assert_eq!(
            Some(common::currency::domain::Currency::Eur),
            subscription.currency()
        );
        assert_eq!(Some(user_id), subscription.user_id());
    }

    #[tokio::test]
    async fn should_prefer_request_fields_over_profile_fallback() {
        let user_id = UserId::new();
        let reader = FakeNewsletterProfileReader::default();
        lock(&reader.state).profile = Some(NewsletterProfile {
            first_name: Some("Profile".into()),
            last_name: Some("Name".into()),
            language: Some(common::language::domain::Language::En),
            currency: Some(common::currency::domain::Currency::Eur),
        });
        let writer = FakeNewsletterSubscriptionWriter::default();
        let command = UpsertNewsletterSubscriptionCommand {
            first_name: Some("Request".into()),
            last_name: Some("Override".into()),
            language: Some(common::language::domain::Language::De),
            currency: Some(common::currency::domain::Currency::Usd),
            ..command()
        };

        let result = UpsertNewsletterSubscriptionHandler::new(reader, writer.clone())
            .execute(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: Default::default(),
                }),
                command,
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&writer.state);
        let subscription = match state.subscription.as_ref() {
            Some(subscription) => subscription,
            None => panic!("expected subscription"),
        };
        assert_eq!(
            Some("Request"),
            subscription.first_name().map(AsRef::as_ref)
        );
        assert_eq!(
            Some("Override"),
            subscription.last_name().map(AsRef::as_ref)
        );
        assert_eq!(
            Some(common::language::domain::Language::De),
            subscription.language()
        );
        assert_eq!(
            Some(common::currency::domain::Currency::Usd),
            subscription.currency()
        );
    }

    #[tokio::test]
    async fn should_continue_when_profile_is_missing_or_fails() {
        for error in [
            None,
            Some(ProfileErrorKind::TemporarilyUnavailable),
            Some(ProfileErrorKind::InvalidReadModel),
            Some(ProfileErrorKind::Internal),
        ] {
            let user_id = UserId::new();
            let reader = FakeNewsletterProfileReader::default();
            lock(&reader.state).error = error;
            let writer = FakeNewsletterSubscriptionWriter::default();

            let result = UpsertNewsletterSubscriptionHandler::new(reader.clone(), writer.clone())
                .execute(&context(Principal::User(user_id)), command())
                .await;

            assert!(result.is_ok());
            assert_eq!(1, lock(&reader.state).calls);
            let state = lock(&writer.state);
            assert_eq!(1, state.calls);
            let subscription = match state.subscription.as_ref() {
                Some(subscription) => subscription,
                None => panic!("expected subscription"),
            };
            assert_eq!(None, subscription.first_name());
            assert_eq!(None, subscription.last_name());
            assert_eq!(None, subscription.language());
            assert_eq!(None, subscription.currency());
            assert_eq!(Some(user_id), subscription.user_id());
        }
    }

    #[tokio::test]
    async fn should_map_invalid_email_writer_error() {
        let reader = FakeNewsletterProfileReader::default();
        let writer = FakeNewsletterSubscriptionWriter::default();
        lock(&writer.state).error = Some(WriterErrorKind::InvalidEmail);

        let result = UpsertNewsletterSubscriptionHandler::new(reader, writer)
            .execute(&context(Principal::Anonymous), command())
            .await;

        assert!(matches!(
            result,
            Err(UpsertNewsletterSubscriptionError::InvalidEmail)
        ));
    }

    #[tokio::test]
    async fn should_map_writer_errors_with_sources() {
        use std::error::Error;

        for kind in [
            WriterErrorKind::TemporarilyUnavailable,
            WriterErrorKind::Internal,
        ] {
            let reader = FakeNewsletterProfileReader::default();
            let writer = FakeNewsletterSubscriptionWriter::default();
            lock(&writer.state).error = Some(kind);

            let result = UpsertNewsletterSubscriptionHandler::new(reader, writer)
                .execute(&context(Principal::Anonymous), command())
                .await;

            match result {
                Err(
                    error @ UpsertNewsletterSubscriptionError::NewsletterSubscriptionUnavailable {
                        ..
                    },
                )
                | Err(
                    error @ UpsertNewsletterSubscriptionError::NewsletterSubscriptionInternal {
                        ..
                    },
                ) => {
                    assert!(error.source().is_some());
                }
                Ok(()) | Err(UpsertNewsletterSubscriptionError::InvalidEmail) => {
                    panic!("unexpected writer error mapping")
                }
            }
        }
    }
}
