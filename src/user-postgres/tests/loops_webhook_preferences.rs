use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::{marketing_consent_sync_intent_id::MarketingConsentSyncIntentId, user_id::UserId};
use user_postgres::{
    SqlxLoopsWebhookReceiptRepository, SqlxMarketingConsentIntentRepository,
    SqlxNewsletterConfirmationChallengesRepository,
};
use user_service::ports::marketing_consent_intents::ConsentWorkerClaimOutcome;
use user_service::ports::{
    ConsentIntent, LoopsWebhookReceiptDisposition as Disposition, LoopsWebhookReceipts,
    LoopsWebhookReceiptsFactory, MarketingEmailConsentError, MarketingEmailConsentOutcome,
    MarketingEmailConsentWriter, MarketingEmailSubscriptionState, NewsletterConfirmationChallenges,
    NewsletterConfirmationChallengesFactory, NewsletterConfirmationIssueOutcome,
    NewsletterConfirmationSendStatus, NewsletterProfile, NewsletterWebhookDeliveryId,
    NewsletterWebhookEmailAddress, NewsletterWebhookEventKind, NewsletterWebhookEventName,
    NewsletterWebhookMailingListId, NewsletterWebhookProviderContactId,
    NewsletterWebhookRawBodySha256, NewsletterWebhookVerification,
    NewsletterWebhookVerificationOutcome, VerifiedNewsletterWebhookEvent,
};
use user_service::use_cases::commands::coordinate_marketing_consent::MarketingConsentCoordinator;
use user_service::use_cases::{
    ApplyLoopsPreferenceEventCommand, ApplyLoopsPreferenceEventHandler,
    ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventUseCase,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const TARGET_LIST: &str = "marketing-list-id";
const CONTACT_ID: &str = "loops-contact-1";

fn email(value: &str) -> Email {
    Email::try_from(value).unwrap()
}

fn timestamp() -> OffsetDateTime {
    OffsetDateTime::now_utc().replace_nanosecond(0).unwrap()
}

async fn seed_user(pool: &sqlx::PgPool, id: UserId, address: &Email) {
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(id.as_uuid())
        .bind::<&str>(address.as_ref())
        .execute(pool)
        .await
        .unwrap();
}

async fn grant_user(pool: &sqlx::PgPool, address: &Email, changed_at: OffsetDateTime) {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    MarketingConsentCoordinator::new(&mut tx, &SqlxMarketingConsentIntentRepository::new())
        .accepted_double_opt_in(
            format!("test-{}", uuid::Uuid::new_v4()),
            address.clone(),
            changed_at,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn claim_grant(pool: &sqlx::PgPool, address: &Email) -> MarketingConsentSyncIntentId {
    let intent_uuid: uuid::Uuid = sqlx::query_scalar(
        "SELECT intent_id FROM marketing_email_consent_sync_intents WHERE email = $1 AND desired ORDER BY intent_sequence LIMIT 1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_one(pool)
    .await
    .unwrap();
    let intent_id = MarketingConsentSyncIntentId::try_from(intent_uuid).unwrap();
    let worker = user_postgres::SqlxMarketingConsentIntentWorker::new();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        worker.claim_by_id(&mut tx, intent_id).await.unwrap(),
        ConsentWorkerClaimOutcome::Claimed(_)
    ));
    tx.commit().await.unwrap();
    intent_id
}

#[expect(
    clippy::too_many_arguments,
    reason = "a verified webhook fixture makes every signed event field explicit"
)]
fn verified_event(
    delivery_id: &str,
    digest: u8,
    event_name: &str,
    kind: NewsletterWebhookEventKind,
    event_time: OffsetDateTime,
    address: &str,
    contact_id: &str,
    list_id: Option<&str>,
) -> NewsletterWebhookVerification {
    NewsletterWebhookVerification {
        raw_body_sha256: NewsletterWebhookRawBodySha256::new([digest; 32]),
        outcome: NewsletterWebhookVerificationOutcome::Verified(VerifiedNewsletterWebhookEvent {
            delivery_id: NewsletterWebhookDeliveryId::new(delivery_id.to_owned()).unwrap(),
            provider_event_name: NewsletterWebhookEventName::new(event_name.to_owned()).unwrap(),
            kind,
            event_time_unix_seconds: event_time.unix_timestamp(),
            provider_contact_id: NewsletterWebhookProviderContactId::new(contact_id.to_owned())
                .unwrap(),
            email: NewsletterWebhookEmailAddress::new(address.to_owned()).unwrap(),
            mailing_list_id: list_id
                .map(|id| NewsletterWebhookMailingListId::new(id.to_owned()).unwrap()),
        }),
    }
}

fn unsupported_event(
    delivery_id: &str,
    digest: u8,
    event_time: OffsetDateTime,
) -> NewsletterWebhookVerification {
    use user_service::ports::IgnoredNewsletterWebhookEvent;
    NewsletterWebhookVerification {
        raw_body_sha256: NewsletterWebhookRawBodySha256::new([digest; 32]),
        outcome: NewsletterWebhookVerificationOutcome::Ignored(IgnoredNewsletterWebhookEvent {
            delivery_id: NewsletterWebhookDeliveryId::new(delivery_id.to_owned()).unwrap(),
            provider_event_name: NewsletterWebhookEventName::new("testing.testEvent".to_owned())
                .unwrap(),
            event_time_unix_seconds: event_time.unix_timestamp(),
        }),
    }
}

struct MockProvider {
    state: MarketingEmailSubscriptionState,
    withdraw_during_read: Option<(sqlx::PgPool, Email, OffsetDateTime)>,
    fail_read: bool,
}

impl MockProvider {
    fn state(
        contact_id: &str,
        globally_subscribed: bool,
        on_target_list: bool,
        suppressed: bool,
    ) -> Self {
        Self {
            state: MarketingEmailSubscriptionState::Present {
                contact_id: contact_id.to_owned(),
                globally_subscribed,
                on_target_list,
                suppressed,
            },
            withdraw_during_read: None,
            fail_read: false,
        }
    }
}

#[async_trait::async_trait]
impl MarketingEmailConsentWriter for MockProvider {
    async fn current_state(
        &self,
        _email: &Email,
    ) -> Result<MarketingEmailSubscriptionState, MarketingEmailConsentError> {
        if self.fail_read {
            return Err(MarketingEmailConsentError::ReadUnavailable);
        }
        if let Some((pool, address, event_at)) = &self.withdraw_during_read {
            let uow = SqlxUnitOfWork::new(pool.clone());
            let mut tx = uow
                .begin()
                .await
                .map_err(|_| MarketingEmailConsentError::ReadUnavailable)?;
            MarketingConsentCoordinator::new(&mut tx, &SqlxMarketingConsentIntentRepository::new())
                .provider_withdrawal(address.clone(), *event_at)
                .await
                .map_err(|_| MarketingEmailConsentError::ReadUnavailable)?;
            SqlxLoopsWebhookReceiptRepository::new()
                .in_transaction(&mut tx)
                .advance_preference_fence(address, CONTACT_ID, *event_at, false)
                .await
                .map_err(|_| MarketingEmailConsentError::ReadUnavailable)?;
            tx.commit()
                .await
                .map_err(|_| MarketingEmailConsentError::ReadUnavailable)?;
        }
        Ok(self.state.clone())
    }

    async fn grant(
        &self,
        _intent: &ConsentIntent,
    ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
        Err(MarketingEmailConsentError::IneligibleIntent)
    }

    async fn revoke(
        &self,
        _intent: &ConsentIntent,
    ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
        Err(MarketingEmailConsentError::IneligibleIntent)
    }
}

fn handler(
    pool: &sqlx::PgPool,
    provider: MockProvider,
) -> ApplyLoopsPreferenceEventHandler<
    SqlxUnitOfWork,
    SqlxMarketingConsentIntentRepository,
    SqlxLoopsWebhookReceiptRepository,
    MockProvider,
> {
    ApplyLoopsPreferenceEventHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxLoopsWebhookReceiptRepository::new(),
        provider,
        NewsletterWebhookMailingListId::new(TARGET_LIST.to_owned()).unwrap(),
    )
}

async fn consent_state(pool: &sqlx::PgPool, id: UserId) -> (bool, i64, Option<OffsetDateTime>) {
    sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, marketing_email_consent_changed_at FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn provider_withdrawal_commits_consent_grant_cancellation_challenge_invalidation_and_receipt_together()
 {
    let pool = get_postgres_client().await;
    let address = email("loops-withdraw@example.test");
    let user_id = UserId::new();
    let other_address = email("loops-other-mailbox@example.test");
    let other_user_id = UserId::new();
    seed_user(&pool, user_id, &address).await;
    seed_user(&pool, other_user_id, &other_address).await;
    let granted_at = timestamp() - Duration::minutes(2);
    grant_user(&pool, &address, granted_at).await;
    let grant_intent_id = claim_grant(&pool, &address).await;
    grant_user(&pool, &other_address, granted_at).await;

    let challenge_id = user_core::newsletter_confirmation_id::NewsletterConfirmationId::new();
    let token =
        user_core::newsletter_confirmation::RawNewsletterConfirmationToken::from_entropy([71; 32]);
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let issue = challenge_repo
        .in_transaction(&mut tx)
        .create_if_allowed(user_service::ports::NewNewsletterConfirmationChallenge {
            id: challenge_id,
            token_digest: token.digest(),
            email: address.clone(),
            requested_by_user_id: Some(user_id),
            profile: NewsletterProfile::default(),
            now: timestamp(),
        })
        .await
        .unwrap();
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, issue);
    challenge_repo
        .in_transaction(&mut tx)
        .record_send_outcome(
            challenge_id,
            NewsletterConfirmationSendStatus::Accepted,
            timestamp(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let event_at = timestamp();
    let outcome = handler(&pool, MockProvider::state(CONTACT_ID, false, false, false))
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-withdraw-1",
                1,
                "contact.mailingList.unsubscribed",
                NewsletterWebhookEventKind::MailingListUnsubscribed,
                event_at,
                address.as_ref(),
                CONTACT_ID,
                Some(TARGET_LIST),
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::CommittedApplication(Disposition::AppliedWithdrawal),
        outcome
    );
    let (consent, revision, changed_at) = consent_state(&pool, user_id).await;
    assert!(!consent);
    assert_eq!(2, revision);
    assert_eq!(Some(event_at), changed_at);
    assert_eq!(
        (true, 1, Some(granted_at)),
        consent_state(&pool, other_user_id).await
    );
    let grant_state: (String, Option<String>) = sqlx::query_as(
        "SELECT status, last_error_code FROM marketing_email_consent_sync_intents WHERE intent_id = $1",
    )
    .bind(grant_intent_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (
            "BLOCKED".to_owned(),
            Some("PROVIDER_WITHDRAWAL_RACE_CANDIDATE".to_owned())
        ),
        grant_state
    );
    let intent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        1, intent_count,
        "provider back-sync does not enqueue an echo or repair"
    );
    let invalidated: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(challenge_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(Some(event_at), invalidated);
    let (disposition, receipt_event_at, expiry_days): (String, OffsetDateTime, i32) = sqlx::query_as(
        "SELECT disposition, event_time, round(extract(epoch FROM (expires_at - processed_at)) / 86400.0)::int FROM loops_webhook_receipts WHERE delivery_id = $1",
    )
    .bind("loops-withdraw-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(Disposition::AppliedWithdrawal.as_str(), disposition);
    assert_eq!(event_at, receipt_event_at);
    assert_eq!(35, expiry_days);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn email_only_withdrawal_cancels_matching_grant_and_pending_challenge_without_creating_user()
{
    let pool = get_postgres_client().await;
    let address = email("loops-email-only-withdraw@example.test");
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let challenge_id = user_core::newsletter_confirmation_id::NewsletterConfirmationId::new();
    let token =
        user_core::newsletter_confirmation::RawNewsletterConfirmationToken::from_entropy([73; 32]);
    let issued_at = timestamp() - Duration::minutes(2);
    let mut tx = uow.begin().await.unwrap();
    let issue = challenge_repo
        .in_transaction(&mut tx)
        .create_if_allowed(user_service::ports::NewNewsletterConfirmationChallenge {
            id: challenge_id,
            token_digest: token.digest(),
            email: address.clone(),
            requested_by_user_id: None,
            profile: NewsletterProfile::default(),
            now: issued_at,
        })
        .await
        .unwrap();
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, issue);
    MarketingConsentCoordinator::new(&mut tx, &SqlxMarketingConsentIntentRepository::new())
        .accepted_double_opt_in(
            "email-only-provider-grant".to_owned(),
            address.clone(),
            issued_at,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let event_at = timestamp();
    let outcome = handler(&pool, MockProvider::state(CONTACT_ID, false, false, false))
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-email-only-withdrawal",
                8,
                "email.unsubscribed",
                NewsletterWebhookEventKind::EmailUnsubscribed,
                event_at,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::CommittedApplication(Disposition::AppliedWithdrawal),
        outcome
    );
    let user_count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
        .bind::<&str>(address.as_ref())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, user_count);
    let intent_state: (String, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT status, user_id FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(("BLOCKED".to_owned(), None), intent_state);
    let invalidated: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(challenge_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(Some(event_at), invalidated);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn unrelated_lists_api_echo_forged_metadata_and_unknown_mailboxes_do_not_grant_or_redirect() {
    let pool = get_postgres_client().await;
    let address = email("loops-existing@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    let granted_at = timestamp() - Duration::minutes(1);
    grant_user(&pool, &address, granted_at).await;
    let handler = handler(&pool, MockProvider::state(CONTACT_ID, true, true, false));
    let now = timestamp();

    let unrelated = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-unrelated-list",
                2,
                "contact.mailingList.unsubscribed",
                NewsletterWebhookEventKind::MailingListUnsubscribed,
                now,
                address.as_ref(),
                CONTACT_ID,
                Some("another-list"),
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredUnrelatedList),
        unrelated
    );
    let echo = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-api-echo",
                3,
                "contact.mailingList.subscribed",
                NewsletterWebhookEventKind::MailingListSubscribed,
                now,
                address.as_ref(),
                CONTACT_ID,
                Some(TARGET_LIST),
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredApiEcho),
        echo
    );

    // The verified event boundary contains no provider userId/custom metadata. An
    // unknown exact email cannot be redirected to this account by those fields.
    let forged_metadata_email = "not-the-account@example.test";
    let ignored = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-forged-user-metadata",
                4,
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                now,
                forged_metadata_email,
                "other-contact",
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredNoRegisteredUser),
        ignored
    );
    assert_eq!((true, 1, Some(granted_at)), consent_state(&pool, id).await);
    let fences: i64 = sqlx::query_scalar("SELECT count(*) FROM loops_webhook_preference_fences")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, fences);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn duplicate_delivery_is_concurrent_idempotent_and_changed_body_reuse_conflicts() {
    let pool = get_postgres_client().await;
    let address = email("loops-duplicate@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    grant_user(&pool, &address, timestamp() - Duration::minutes(1)).await;
    let handler = Arc::new(handler(
        &pool,
        MockProvider::state(CONTACT_ID, false, false, false),
    ));
    let event_time = timestamp();
    let command = || ApplyLoopsPreferenceEventCommand {
        verification: verified_event(
            "loops-duplicate-delivery",
            5,
            "email.unsubscribed",
            NewsletterWebhookEventKind::EmailUnsubscribed,
            event_time,
            address.as_ref(),
            CONTACT_ID,
            None,
        ),
    };
    let (first, second) = tokio::join!(handler.execute(command()), handler.execute(command()));
    let results = [first.unwrap(), second.unwrap()];
    assert!(
        results.contains(&ApplyLoopsPreferenceEventOutcome::CommittedApplication(
            Disposition::AppliedWithdrawal
        ))
    );
    assert!(results.contains(&ApplyLoopsPreferenceEventOutcome::Duplicate));
    assert_eq!((false, 2, Some(event_time)), consent_state(&pool, id).await);

    let conflict = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-duplicate-delivery",
                6,
                "email.unsubscribed",
                NewsletterWebhookEventKind::EmailUnsubscribed,
                event_time + Duration::seconds(1),
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(ApplyLoopsPreferenceEventOutcome::Conflict, conflict);
    assert_eq!((false, 2, Some(event_time)), consent_state(&pool, id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn failed_receipt_insert_rolls_back_user_intent_and_challenge_effects() {
    let pool = get_postgres_client().await;
    let address = email("loops-atomic-rollback@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    let granted_at = timestamp() - Duration::minutes(2);
    grant_user(&pool, &address, granted_at).await;
    let challenge_id = user_core::newsletter_confirmation_id::NewsletterConfirmationId::new();
    let token =
        user_core::newsletter_confirmation::RawNewsletterConfirmationToken::from_entropy([72; 32]);
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    challenge_repo
        .in_transaction(&mut tx)
        .create_if_allowed(user_service::ports::NewNewsletterConfirmationChallenge {
            id: challenge_id,
            token_digest: token.digest(),
            email: address.clone(),
            requested_by_user_id: Some(id),
            profile: NewsletterProfile::default(),
            now: timestamp(),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    sqlx::query("CREATE OR REPLACE FUNCTION reject_loops_receipt_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected receipt failure'; END $$")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_loops_receipt_insert BEFORE INSERT ON loops_webhook_receipts FOR EACH ROW EXECUTE FUNCTION reject_loops_receipt_insert()")
        .execute(&pool)
        .await
        .unwrap();
    let event_time = timestamp();
    let outcome = handler(&pool, MockProvider::state(CONTACT_ID, false, false, false))
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-rollback",
                7,
                "contact.unsubscribed",
                NewsletterWebhookEventKind::ContactUnsubscribed,
                event_time,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await;
    assert!(outcome.is_err());
    sqlx::query("DROP TRIGGER reject_loops_receipt_insert ON loops_webhook_receipts")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_loops_receipt_insert()")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!((true, 1, Some(granted_at)), consent_state(&pool, id).await);
    let status: String = sqlx::query_scalar(
        "SELECT status FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!("PENDING", status);
    let invalidated: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(challenge_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(None, invalidated);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM loops_webhook_receipts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn resubscribe_needs_both_current_provider_scopes_and_never_overrides_newer_withdrawal() {
    let pool = get_postgres_client().await;
    let event_base = timestamp();
    for (index, state) in [
        MockProvider::state(CONTACT_ID, true, false, false),
        MockProvider::state(CONTACT_ID, false, true, false),
        MockProvider::state(CONTACT_ID, true, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let address = email(&format!("loops-blocked-{index}@example.test"));
        let id = UserId::new();
        seed_user(&pool, id, &address).await;
        let result = handler(&pool, state)
            .execute(ApplyLoopsPreferenceEventCommand {
                verification: verified_event(
                    &format!("loops-blocked-{index}"),
                    20 + index as u8,
                    "email.resubscribed",
                    NewsletterWebhookEventKind::EmailResubscribed,
                    event_base,
                    address.as_ref(),
                    CONTACT_ID,
                    None,
                ),
            })
            .await
            .unwrap();
        assert_eq!(
            ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredProviderState),
            result
        );
        assert_eq!((false, 0, None), consent_state(&pool, id).await);
    }

    let stale_address = email("loops-stale-provider-read@example.test");
    let stale_id = UserId::new();
    seed_user(&pool, stale_id, &stale_address).await;
    let racing_state = MockProvider {
        state: MarketingEmailSubscriptionState::Present {
            contact_id: CONTACT_ID.to_owned(),
            globally_subscribed: true,
            on_target_list: true,
            suppressed: false,
        },
        withdraw_during_read: Some((
            pool.clone(),
            stale_address.clone(),
            event_base + Duration::seconds(1),
        )),
        fail_read: false,
    };
    let stale = handler(&pool, racing_state)
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-provider-read-race",
                30,
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                event_base,
                stale_address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredStale),
        stale
    );
    assert_eq!((false, 0, None), consent_state(&pool, stale_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn later_verified_resubscribe_updates_only_exact_registered_mailbox_without_outbound_echo() {
    let pool = get_postgres_client().await;
    let address = email("loops-resubscribe@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    let now = timestamp();
    let result = handler(&pool, MockProvider::state(CONTACT_ID, true, true, false))
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-valid-resubscribe",
                40,
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                now,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::CommittedApplication(Disposition::AppliedResubscription),
        result
    );
    assert_eq!((true, 1, Some(now)), consent_state(&pool, id).await);
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(0, intents);
    let fence: (OffsetDateTime, String, bool) = sqlx::query_as(
        "SELECT latest_event_at, provider_contact_id, purpose_subscribed FROM loops_webhook_preference_fences WHERE email = $1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((now, CONTACT_ID.to_owned(), true), fence);

    let unrelated_address = email("loops-unregistered-resubscribe@example.test");
    let no_user = handler(&pool, MockProvider::state(CONTACT_ID, true, true, false))
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-email-only-resubscribe",
                41,
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                now,
                unrelated_address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredNoRegisteredUser),
        no_user
    );
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
        .bind::<&str>(unrelated_address.as_ref())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, users);

    let future = handler(&pool, MockProvider::state(CONTACT_ID, true, true, false))
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-future-resubscribe",
                43,
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                now + Duration::seconds(2),
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredUntrustworthyTime),
        future
    );
    assert_eq!((true, 1, Some(now)), consent_state(&pool, id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn uncertain_provider_read_does_not_commit_receipt_or_local_permission() {
    let pool = get_postgres_client().await;
    let address = email("loops-provider-read-error@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    let now = timestamp();
    let provider = MockProvider {
        state: MarketingEmailSubscriptionState::Present {
            contact_id: CONTACT_ID.to_owned(),
            globally_subscribed: true,
            on_target_list: true,
            suppressed: false,
        },
        withdraw_during_read: None,
        fail_read: true,
    };
    let outcome = handler(&pool, provider)
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-provider-read-error",
                42,
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                now,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await;
    assert!(matches!(
        outcome,
        Err(user_service::use_cases::ApplyLoopsPreferenceEventError::Retryable)
    ));
    assert_eq!((false, 0, None), consent_state(&pool, id).await);
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM loops_webhook_receipts WHERE email = $1")
            .bind::<&str>(address.as_ref())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(0, receipts);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn stale_and_equal_provider_positive_events_cannot_reverse_a_withdrawal() {
    let pool = get_postgres_client().await;
    let address = email("loops-ordering@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    let granted_at = timestamp() - Duration::minutes(2);
    grant_user(&pool, &address, granted_at).await;
    let withdrawal_at = timestamp();
    let handler = handler(&pool, MockProvider::state(CONTACT_ID, true, true, false));
    let withdrawn = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-ordering-negative",
                50,
                "contact.unsubscribed",
                NewsletterWebhookEventKind::ContactUnsubscribed,
                withdrawal_at,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::CommittedApplication(Disposition::AppliedWithdrawal),
        withdrawn
    );
    for (id_suffix, event_at) in [
        ("old", withdrawal_at - Duration::seconds(1)),
        ("equal", withdrawal_at),
    ] {
        let positive = handler
            .execute(ApplyLoopsPreferenceEventCommand {
                verification: verified_event(
                    &format!("loops-ordering-{id_suffix}"),
                    if id_suffix == "old" { 51 } else { 52 },
                    "email.resubscribed",
                    NewsletterWebhookEventKind::EmailResubscribed,
                    event_at,
                    address.as_ref(),
                    CONTACT_ID,
                    None,
                ),
            })
            .await
            .unwrap();
        assert_eq!(
            ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredStale),
            positive
        );
    }
    assert_eq!(
        (false, 2, Some(withdrawal_at)),
        consent_state(&pool, id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn hard_bounce_is_not_a_consent_withdrawal_and_complaint_keeps_its_source_label() {
    let pool = get_postgres_client().await;
    let address = email("loops-delivery-signals@example.test");
    let id = UserId::new();
    seed_user(&pool, id, &address).await;
    let granted_at = timestamp() - Duration::minutes(1);
    grant_user(&pool, &address, granted_at).await;
    let handler = handler(&pool, MockProvider::state(CONTACT_ID, true, true, true));
    let bounce_at = timestamp();
    let bounce = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-hard-bounce",
                70,
                "email.hardBounced",
                NewsletterWebhookEventKind::EmailHardBounced,
                bounce_at,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredHardBounce),
        bounce
    );
    assert_eq!((true, 1, Some(granted_at)), consent_state(&pool, id).await);

    let complaint_at = bounce_at + Duration::seconds(1);
    let complaint = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: verified_event(
                "loops-spam-complaint",
                71,
                "email.spamReported",
                NewsletterWebhookEventKind::EmailSpamReported,
                complaint_at,
                address.as_ref(),
                CONTACT_ID,
                None,
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::CommittedApplication(Disposition::AppliedComplaintBlock),
        complaint
    );
    assert_eq!(
        (false, 2, Some(complaint_at)),
        consent_state(&pool, id).await
    );
    let (event_name, disposition): (String, String) = sqlx::query_as(
        "SELECT provider_event_name, disposition FROM loops_webhook_receipts WHERE delivery_id = $1",
    )
    .bind("loops-spam-complaint")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!("email.spamReported", event_name);
    assert_eq!(Disposition::AppliedComplaintBlock.as_str(), disposition);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn verified_unsupported_event_is_receipted_and_delivery_id_reuse_conflicts() {
    let pool = get_postgres_client().await;
    let now = timestamp();
    let handler = handler(&pool, MockProvider::state(CONTACT_ID, false, false, false));
    let first = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: unsupported_event("loops-unsupported", 60, now),
        })
        .await
        .unwrap();
    assert_eq!(
        ApplyLoopsPreferenceEventOutcome::Ignored(Disposition::IgnoredUnsupportedEvent),
        first
    );
    let conflict = handler
        .execute(ApplyLoopsPreferenceEventCommand {
            verification: unsupported_event("loops-unsupported", 61, now),
        })
        .await
        .unwrap();
    assert_eq!(ApplyLoopsPreferenceEventOutcome::Conflict, conflict);
    let saved_name: String = sqlx::query_scalar(
        "SELECT provider_event_name FROM loops_webhook_receipts WHERE delivery_id = $1",
    )
    .bind("loops-unsupported")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!("testing.testEvent", saved_name);
}
