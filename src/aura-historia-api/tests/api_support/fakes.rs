//! Acceptance-only authentication, failure injection and captured effects.
use super::*;

pub(super) const TEST_COGNITO_ISSUER: &str = "https://issuer.api-acceptance.test/pool";

pub(super) static COGNITO_SUBJECTS_BY_USER: OnceLock<Mutex<HashMap<UserId, CognitoSubject>>> =
    OnceLock::new();
static SESSION_REVOCATION_FAILURES: OnceLock<Mutex<HashSet<CognitoSubject>>> = OnceLock::new();
static NEWSLETTER_CONFIRMATION_TOKENS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

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

#[derive(Clone, Copy)]
struct CapturingNewsletterConfirmationEmailSender;

#[async_trait::async_trait]
impl user_service::ports::NewsletterConfirmationEmailSender
    for CapturingNewsletterConfirmationEmailSender
{
    async fn send(
        &self,
        email: user_service::ports::NewsletterConfirmationEmail,
    ) -> user_service::ports::NewsletterConfirmationEmailSendOutcome {
        let tokens = NEWSLETTER_CONFIRMATION_TOKENS.get_or_init(|| Mutex::new(HashMap::new()));
        tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(email.recipient.to_string(), email.token.as_str().to_owned());
        user_service::ports::NewsletterConfirmationEmailSendOutcome::Accepted
    }
}

pub fn take_newsletter_confirmation_token(email: &str) -> String {
    let tokens = NEWSLETTER_CONFIRMATION_TOKENS.get_or_init(|| Mutex::new(HashMap::new()));
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
            CapturingNewsletterConfirmationEmailSender,
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
    let subjects = COGNITO_SUBJECTS_BY_USER.get_or_init(|| Mutex::new(HashMap::new()));
    let subject = match subjects.lock() {
        Ok(subjects) => subjects.get(&user_id).cloned(),
        Err(poisoned) => poisoned.into_inner().get(&user_id).cloned(),
    }
    .unwrap_or_else(|| panic!("missing Cognito identity fixture for {user_id}"));
    let failures = SESSION_REVOCATION_FAILURES.get_or_init(|| Mutex::new(HashSet::new()));
    match failures.lock() {
        Ok(mut failures) => {
            failures.insert(subject);
        }
        Err(poisoned) => {
            poisoned.into_inner().insert(subject);
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct SuccessfulUserSessionRevoker;

#[async_trait::async_trait]
impl UserSessionRevoker for SuccessfulUserSessionRevoker {
    async fn revoke_sessions(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<(), UserSessionRevocationError> {
        let failures = SESSION_REVOCATION_FAILURES.get_or_init(|| Mutex::new(HashSet::new()));
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

static WOOCOMMERCE_INGESTION_MESSAGES: OnceLock<Mutex<Vec<ProductListingIngestionMessage>>> =
    OnceLock::new();
static WOOCOMMERCE_PUBLISHER_OUTCOMES: OnceLock<
    Mutex<HashMap<ListingSourceId, ProductListingIngestionOutcome>>,
> = OnceLock::new();

fn woocommerce_messages() -> &'static Mutex<Vec<ProductListingIngestionMessage>> {
    WOOCOMMERCE_INGESTION_MESSAGES.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn woocommerce_ingestion_messages(
    listing_source_id: ListingSourceId,
) -> Vec<ProductListingIngestionMessage> {
    captured_ingestion_messages(listing_source_id)
}

pub fn captured_ingestion_messages(
    listing_source_id: ListingSourceId,
) -> Vec<ProductListingIngestionMessage> {
    woocommerce_messages()
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
    WOOCOMMERCE_PUBLISHER_OUTCOMES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(listing_source_id, outcome);
}

pub(super) struct RecordingWoocommerceIngestionPublisher;

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for RecordingWoocommerceIngestionPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        tokio::task::yield_now().await;
        let outcomes = WOOCOMMERCE_PUBLISHER_OUTCOMES.get_or_init(|| Mutex::new(HashMap::new()));
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
        woocommerce_messages()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(commands);
        Ok(items)
    }
}

static API_REQUESTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn api_request_count() -> usize {
    API_REQUESTS.load(std::sync::atomic::Ordering::SeqCst)
}

pub(super) async fn count_api_request(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    API_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    next.run(request).await
}
