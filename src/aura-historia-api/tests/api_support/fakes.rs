//! Acceptance-only authentication, failure injection and captured effects.
use super::*;

pub(super) const TEST_COGNITO_ISSUER: &str = "https://issuer.api-acceptance.test/pool";

#[derive(Default)]
pub(super) struct ApiTestState {
    cognito_subjects_by_user: Mutex<HashMap<UserId, CognitoSubject>>,
    session_revocation_failures: Mutex<HashSet<CognitoSubject>>,
    newsletter_confirmation_tokens: Mutex<HashMap<String, String>>,
    ingestion_messages: Mutex<Vec<ProductListingIngestionMessage>>,
    publisher_outcomes: Mutex<HashMap<ListingSourceId, ProductListingIngestionOutcome>>,
    requests: std::sync::atomic::AtomicUsize,
    postgres_pools: Mutex<HashMap<tokio::runtime::Id, Arc<tokio::sync::OnceCell<sqlx::PgPool>>>>,
}

pub struct ApiTestStateFixture;
pub const API_TEST_STATE: ApiTestStateFixture = ApiTestStateFixture;

#[async_trait::async_trait]
impl test_api::IntegrationTestService for ApiTestStateFixture {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        test_api::TestContext::current()
            .expect("API acceptance context")
            .insert_api_state(Arc::new(ApiTestState::default()))
            .expect("initialize API acceptance state before servers");
    }
}

pub(super) fn api_test_state() -> Arc<ApiTestState> {
    test_api::TestContext::current()
        .expect("API acceptance context")
        .api_state::<Arc<ApiTestState>>()
        .expect("API acceptance state fixture")
}

// A context spans the test runtime and its HTTP server runtime. Reuse within each runtime
// without keeping one connection per seed helper or moving SQLx sockets between runtimes.
pub(super) async fn postgres_pool() -> sqlx::PgPool {
    let state = api_test_state();
    let runtime = tokio::runtime::Handle::current().id();
    let pool = state
        .postgres_pools
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(runtime)
        .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
        .clone();
    pool.get_or_init(test_api::get_postgres_client)
        .await
        .clone()
}

pub(super) fn record_cognito_subject(user_id: UserId, subject: CognitoSubject) {
    api_test_state()
        .cognito_subjects_by_user
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(user_id, subject);
}

#[derive(Clone, Copy)]
struct NewsletterC09TestAuthenticator;

#[async_trait::async_trait]
impl TokenAuthenticator for NewsletterC09TestAuthenticator {
    async fn authenticate(
        &self,
        _bearer_token: &str,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        Err(AuthError::InvalidCredentials)
    }
}

struct CapturingNewsletterConfirmationEmailSender(Arc<ApiTestState>);

#[async_trait::async_trait]
impl user_service::ports::NewsletterConfirmationEmailSender
    for CapturingNewsletterConfirmationEmailSender
{
    async fn send(
        &self,
        email: user_service::ports::NewsletterConfirmationEmail,
    ) -> user_service::ports::NewsletterConfirmationEmailSendOutcome {
        let tokens = &self.0.newsletter_confirmation_tokens;
        tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(email.recipient.to_string(), email.token.as_str().to_owned());
        user_service::ports::NewsletterConfirmationEmailSendOutcome::Accepted
    }
}

pub fn take_newsletter_confirmation_token(email: &str) -> String {
    let state = api_test_state();
    let tokens = &state.newsletter_confirmation_tokens;
    tokens
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(email)
        .unwrap_or_else(|| panic!("missing newsletter confirmation email test fixture"))
}

pub fn newsletter_c09_api_app() -> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let challenges = user_postgres::SqlxNewsletterConfirmationChallengesRepository::new();
        let request = RequestNewsletterSubscriptionHandler::new(
            unit_of_work.clone(),
            challenges,
            user_postgres::SqlxNewsletterProfileReader::new(pool),
            CapturingNewsletterConfirmationEmailSender(api_test_state()),
            user_service::ports::OsNewsletterConfirmationTokenGenerator,
            user_service::ports::SystemNewsletterConfirmationClock,
        );
        let confirm = ConfirmNewsletterSubscriptionHandler::new(
            unit_of_work,
            user_postgres::SqlxNewsletterConfirmationChallengesRepository::new(),
            user_postgres::SqlxMarketingConsentIntentRepository::new(),
            user_service::ports::SystemNewsletterConfirmationClock,
        );
        let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(NewsletterC09TestAuthenticator);
        app(state::AppState::new().with_newsletter(NewsletterState::new(
            Arc::new(request),
            Arc::new(confirm),
            authenticator,
        )))
    })
}

pub fn fail_session_revocation_for(user_id: UserId) {
    let state = api_test_state();
    let subjects = &state.cognito_subjects_by_user;
    let subject = match subjects.lock() {
        Ok(subjects) => subjects.get(&user_id).cloned(),
        Err(poisoned) => poisoned.into_inner().get(&user_id).cloned(),
    }
    .unwrap_or_else(|| panic!("missing Cognito identity fixture for {user_id}"));
    let failures = &state.session_revocation_failures;
    match failures.lock() {
        Ok(mut failures) => {
            failures.insert(subject);
        }
        Err(poisoned) => {
            poisoned.into_inner().insert(subject);
        }
    }
}

pub(super) struct SuccessfulUserSessionRevoker(Arc<ApiTestState>);
impl SuccessfulUserSessionRevoker {
    pub(super) fn new() -> Self {
        Self(api_test_state())
    }
}

#[async_trait::async_trait]
impl UserSessionRevoker for SuccessfulUserSessionRevoker {
    async fn revoke_sessions(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<(), UserSessionRevocationError> {
        let failures = &self.0.session_revocation_failures;
        let should_fail = match failures.lock() {
            Ok(mut failures) => failures.remove(&identity.subject),
            Err(poisoned) => poisoned.into_inner().remove(&identity.subject),
        };
        if should_fail {
            return Err(UserSessionRevocationError::TemporarilyUnavailable {
                source: application::error::box_error(std::io::Error::other("Cognito unavailable")),
            });
        }
        Ok(())
    }
}

pub fn woocommerce_ingestion_messages(
    listing_source_id: ListingSourceId,
) -> Vec<ProductListingIngestionMessage> {
    captured_ingestion_messages(listing_source_id)
}

pub fn captured_ingestion_messages(
    listing_source_id: ListingSourceId,
) -> Vec<ProductListingIngestionMessage> {
    api_test_state()
        .ingestion_messages
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .filter(|message| message.metadata.listing_source_id == listing_source_id)
        .cloned()
        .collect()
}

pub fn set_woocommerce_publisher_outcome(
    listing_source_id: ListingSourceId,
    outcome: ProductListingIngestionOutcome,
) {
    api_test_state()
        .publisher_outcomes
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(listing_source_id, outcome);
}

pub(super) struct RecordingWoocommerceIngestionPublisher(Arc<ApiTestState>);
impl RecordingWoocommerceIngestionPublisher {
    pub(super) fn new() -> Self {
        Self(api_test_state())
    }
}

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for RecordingWoocommerceIngestionPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        tokio::task::yield_now().await;
        let outcomes = &self.0.publisher_outcomes;
        let outcomes = outcomes.lock().unwrap_or_else(|error| error.into_inner());
        let items = commands
            .iter()
            .map(|command| ProductListingIngestionItemOutcome {
                index: command.metadata.index,
                command_id: command.metadata.command_id.clone(),
                outcome: outcomes
                    .get(&command.metadata.listing_source_id)
                    .cloned()
                    .unwrap_or(ProductListingIngestionOutcome::Accepted),
            })
            .collect();
        self.0
            .ingestion_messages
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(commands);
        Ok(items)
    }
}

pub fn api_request_count() -> usize {
    api_test_state()
        .requests
        .load(std::sync::atomic::Ordering::SeqCst)
}

pub(super) async fn count_api_request(
    axum::extract::State(state): axum::extract::State<Arc<ApiTestState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    state
        .requests
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    next.run(request).await
}
