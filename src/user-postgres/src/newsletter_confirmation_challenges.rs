use crate::mapping::{parse_optional_currency, parse_optional_language};
use crate::marketing_consent_intents::{MarketingConsentPersistenceError, lock_recipient};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use serde_email::Email;
use sqlx::{AssertSqlSafe, FromRow};
use time::{Duration, OffsetDateTime};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::newsletter_confirmation::NewsletterConfirmationTokenDigest;
use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
use user_core::user_id::UserId;
use user_service::ports::{
    NewNewsletterConfirmationChallenge, NewsletterConfirmationChallenge,
    NewsletterConfirmationChallengeError, NewsletterConfirmationChallenges,
    NewsletterConfirmationChallengesFactory, NewsletterConfirmationIssueOutcome,
    NewsletterConfirmationSendStatus, marketing_consent_recipient_key,
};
use uuid::Uuid;

const TABLE: &str = "newsletter_subscription_confirmations";
const COLUMNS: &str = "confirmation_id, token_digest, email, recipient_key, purpose, bound_user_id, requester_user_id, first_name, last_name, language, currency, created_at, expires_at, send_attempt_status, confirmed_at, invalidated_at, resulting_intent_id";
const MAX_CLEANUP_BATCH_SIZE: u16 = 1_000;

#[derive(FromRow)]
struct ChallengeRow {
    confirmation_id: Uuid,
    token_digest: Vec<u8>,
    email: String,
    recipient_key: String,
    purpose: String,
    bound_user_id: Option<Uuid>,
    requester_user_id: Option<Uuid>,
    first_name: Option<String>,
    last_name: Option<String>,
    language: Option<String>,
    currency: Option<String>,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    send_attempt_status: String,
    confirmed_at: Option<OffsetDateTime>,
    invalidated_at: Option<OffsetDateTime>,
    resulting_intent_id: Option<Uuid>,
}

impl TryFrom<ChallengeRow> for NewsletterConfirmationChallenge {
    type Error = NewsletterConfirmationChallengeError;

    fn try_from(row: ChallengeRow) -> Result<Self, Self::Error> {
        let email = Email::try_from(row.email)
            .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?;
        if row.purpose != "EMAIL_MARKETING"
            || row.recipient_key != marketing_consent_recipient_key(&email)
            || row.expires_at != row.created_at + Duration::hours(24)
            || row.confirmed_at.is_some() != row.resulting_intent_id.is_some()
            || (row.confirmed_at.is_some() && row.invalidated_at.is_some())
        {
            return Err(NewsletterConfirmationChallengeError::InvalidPersistedState);
        }
        let status = match row.send_attempt_status.as_str() {
            "NOT_ATTEMPTED" => NewsletterConfirmationSendStatus::NotAttempted,
            "ACCEPTED" => NewsletterConfirmationSendStatus::Accepted,
            "DEFINITELY_REJECTED" => NewsletterConfirmationSendStatus::DefinitelyRejected,
            "ACCEPTANCE_UNKNOWN" => NewsletterConfirmationSendStatus::AcceptanceUnknown,
            _ => return Err(NewsletterConfirmationChallengeError::InvalidPersistedState),
        };
        let profile = user_service::ports::NewsletterProfile {
            first_name: row.first_name.map(FirstName::from),
            last_name: row.last_name.map(LastName::from),
            language: parse_optional_language(row.language.as_deref())
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
            currency: parse_optional_currency(row.currency.as_deref())
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
        };
        Ok(Self {
            id: NewsletterConfirmationId::try_from(row.confirmation_id)
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
            token_digest: NewsletterConfirmationTokenDigest::try_from_bytes(&row.token_digest)
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
            email,
            recipient_key: row.recipient_key,
            bound_user_id: row
                .bound_user_id
                .map(UserId::try_from)
                .transpose()
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
            requested_by_user_id: row
                .requester_user_id
                .map(UserId::try_from)
                .transpose()
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
            profile,
            created_at: row.created_at,
            expires_at: row.expires_at,
            send_status: status,
            confirmed_at: row.confirmed_at,
            invalidated_at: row.invalidated_at,
            resulting_intent_id: row
                .resulting_intent_id
                .map(MarketingConsentSyncIntentId::try_from)
                .transpose()
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?,
        })
    }
}

#[derive(Default, Clone, Copy)]
pub struct SqlxNewsletterConfirmationChallengesRepository;

impl SqlxNewsletterConfirmationChallengesRepository {
    pub fn new() -> Self {
        Self
    }
}

pub(crate) async fn invalidate_pending_newsletter_confirmations(
    tx: &mut SqlxTransaction,
    email: &Email,
    invalidated_at: OffsetDateTime,
) -> Result<(), NewsletterConfirmationChallengeError> {
    let recipient_key = marketing_consent_recipient_key(email);
    let conn = tx.connection();
    lock_recipient(&mut *conn, &recipient_key)
        .await
        .map_err(map_consent_persistence_error)?;
    sqlx::query(AssertSqlSafe(format!(
        "UPDATE {TABLE} SET invalidated_at = $3 WHERE recipient_key = $1 AND email = $2 AND confirmed_at IS NULL AND invalidated_at IS NULL"
    )))
    .bind(recipient_key)
    .bind::<&str>(email.as_ref())
    .bind(invalidated_at)
    .execute(&mut *conn)
    .await
    .map_err(db)?;
    Ok(())
}

struct SqlxNewsletterConfirmationChallenges<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl NewsletterConfirmationChallengesFactory<SqlxTransaction>
    for SqlxNewsletterConfirmationChallengesRepository
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl NewsletterConfirmationChallenges + 'tx {
        SqlxNewsletterConfirmationChallenges { tx }
    }
}

#[async_trait::async_trait]
impl NewsletterConfirmationChallenges for SqlxNewsletterConfirmationChallenges<'_> {
    async fn create_if_allowed(
        &mut self,
        challenge: NewNewsletterConfirmationChallenge,
    ) -> Result<NewsletterConfirmationIssueOutcome, NewsletterConfirmationChallengeError> {
        let recipient_key = marketing_consent_recipient_key(&challenge.email);
        let conn = self.tx.connection();
        lock_recipient(&mut *conn, &recipient_key)
            .await
            .map_err(map_consent_persistence_error)?;

        let cooldown_boundary = challenge.now - Duration::minutes(5);
        let recent_issuance: bool = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT EXISTS (SELECT 1 FROM {TABLE} WHERE recipient_key = $1 AND email = $2 AND created_at > $3)"
        )))
        .bind(&recipient_key)
        .bind::<&str>(challenge.email.as_ref())
        .bind(cooldown_boundary)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        if recent_issuance {
            return Ok(NewsletterConfirmationIssueOutcome::Suppressed);
        }

        let budget_boundary = challenge.now - Duration::hours(24);
        let recent_attempts: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT count(*) FROM {TABLE} WHERE recipient_key = $1 AND email = $2 AND created_at > $3"
        )))
        .bind(&recipient_key)
        .bind::<&str>(challenge.email.as_ref())
        .bind(budget_boundary)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        if recent_attempts >= 3 {
            return Ok(NewsletterConfirmationIssueOutcome::Suppressed);
        }

        let bound_user_id: Option<Uuid> =
            sqlx::query_scalar("SELECT user_id FROM users WHERE email = $1")
                .bind::<&str>(challenge.email.as_ref())
                .fetch_optional(&mut *conn)
                .await
                .map_err(db)?;
        if let Some(user_id) = bound_user_id {
            UserId::try_from(user_id)
                .map_err(|_| NewsletterConfirmationChallengeError::InvalidPersistedState)?;
        }

        let first_name = challenge.profile.first_name.as_deref();
        let last_name = challenge.profile.last_name.as_deref();
        let language = challenge
            .profile
            .language
            .map(localization::Language::as_str);
        let currency = challenge.profile.currency.map(money::Currency::as_str);
        sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO {TABLE} (confirmation_id, token_digest, email, recipient_key, purpose, bound_user_id, requester_user_id, first_name, last_name, language, currency, created_at, expires_at) VALUES ($1, $2, $3, $4, 'EMAIL_MARKETING', $5, $6, $7, $8, $9, $10, $11, $12)"
        )))
        .bind(challenge.id.into_uuid())
        .bind(challenge.token_digest.as_bytes().as_slice())
        .bind::<&str>(challenge.email.as_ref())
        .bind(recipient_key)
        .bind(bound_user_id)
        .bind(challenge.requested_by_user_id.map(UserId::into_uuid))
        .bind(first_name)
        .bind(last_name)
        .bind(language)
        .bind(currency)
        .bind(challenge.now)
        .bind(challenge.now + Duration::hours(24))
        .execute(&mut *conn)
        .await
        .map_err(db)?;

        Ok(NewsletterConfirmationIssueOutcome::Issued)
    }

    async fn record_send_outcome(
        &mut self,
        id: NewsletterConfirmationId,
        outcome: NewsletterConfirmationSendStatus,
        recorded_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError> {
        if outcome == NewsletterConfirmationSendStatus::NotAttempted {
            return Err(NewsletterConfirmationChallengeError::InvalidInput);
        }
        let value = send_status_as_str(outcome);
        let result = sqlx::query(AssertSqlSafe(format!(
            "UPDATE {TABLE} SET send_attempt_status = $2, invalidated_at = CASE WHEN $2 = 'DEFINITELY_REJECTED' AND confirmed_at IS NULL THEN COALESCE(invalidated_at, $3) ELSE invalidated_at END WHERE confirmation_id = $1 AND send_attempt_status = 'NOT_ATTEMPTED'"
        )))
        .bind(id.into_uuid())
        .bind(value)
        .bind(recorded_at)
        .execute(self.tx.connection())
        .await
        .map_err(db)?;
        if result.rows_affected() != 1 {
            return Err(NewsletterConfirmationChallengeError::InvalidPersistedState);
        }
        Ok(())
    }

    async fn find_by_token_digest(
        &mut self,
        digest: NewsletterConfirmationTokenDigest,
    ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError> {
        let row = sqlx::query_as::<_, ChallengeRow>(AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM {TABLE} WHERE token_digest = $1"
        )))
        .bind(digest.as_bytes().as_slice())
        .fetch_optional(self.tx.connection())
        .await
        .map_err(db)?;
        row.map(TryInto::try_into).transpose()
    }

    async fn lock_for_confirmation(
        &mut self,
        id: NewsletterConfirmationId,
        digest: NewsletterConfirmationTokenDigest,
    ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError> {
        let recipient_key: Option<String> = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT recipient_key FROM {TABLE} WHERE confirmation_id = $1 AND token_digest = $2"
        )))
        .bind(id.into_uuid())
        .bind(digest.as_bytes().as_slice())
        .fetch_optional(self.tx.connection())
        .await
        .map_err(db)?;
        let Some(recipient_key) = recipient_key else {
            return Ok(None);
        };
        let conn = self.tx.connection();
        lock_recipient(&mut *conn, &recipient_key)
            .await
            .map_err(map_consent_persistence_error)?;
        sqlx::query_as::<_, ChallengeRow>(AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM {TABLE} WHERE confirmation_id = $1 AND token_digest = $2 FOR UPDATE"
        )))
        .bind(id.into_uuid())
        .bind(digest.as_bytes().as_slice())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db)?
        .map(TryInto::try_into)
        .transpose()
    }

    async fn invalidate(
        &mut self,
        id: NewsletterConfirmationId,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError> {
        let result = sqlx::query(AssertSqlSafe(format!(
            "UPDATE {TABLE} SET invalidated_at = $2 WHERE confirmation_id = $1 AND confirmed_at IS NULL AND invalidated_at IS NULL"
        )))
        .bind(id.into_uuid())
        .bind(invalidated_at)
        .execute(self.tx.connection())
        .await
        .map_err(db)?;
        if result.rows_affected() == 0 {
            let found: bool = sqlx::query_scalar(AssertSqlSafe(format!(
                "SELECT EXISTS (SELECT 1 FROM {TABLE} WHERE confirmation_id = $1 AND (confirmed_at IS NOT NULL OR invalidated_at IS NOT NULL))"
            )))
            .bind(id.into_uuid())
            .fetch_one(self.tx.connection())
            .await
            .map_err(db)?;
            if !found {
                return Err(NewsletterConfirmationChallengeError::InvalidPersistedState);
            }
        }
        Ok(())
    }

    async fn complete_confirmation(
        &mut self,
        id: NewsletterConfirmationId,
        bound_user_id: Option<UserId>,
        intent_id: MarketingConsentSyncIntentId,
        confirmed_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError> {
        let conn = self.tx.connection();
        let result = sqlx::query(AssertSqlSafe(format!(
            "UPDATE {TABLE} SET bound_user_id = COALESCE(bound_user_id, $2), confirmed_at = $3, resulting_intent_id = $4 WHERE confirmation_id = $1 AND confirmed_at IS NULL AND invalidated_at IS NULL AND expires_at > $3"
        )))
        .bind(id.into_uuid())
        .bind(bound_user_id.map(UserId::into_uuid))
        .bind(confirmed_at)
        .bind(intent_id.into_uuid())
        .execute(&mut *conn)
        .await
        .map_err(db)?;
        if result.rows_affected() != 1 {
            return Err(NewsletterConfirmationChallengeError::InvalidPersistedState);
        }
        sqlx::query(AssertSqlSafe(format!(
            "UPDATE {TABLE} SET invalidated_at = $2 WHERE recipient_key = (SELECT recipient_key FROM {TABLE} WHERE confirmation_id = $1) AND email = (SELECT email FROM {TABLE} WHERE confirmation_id = $1) AND confirmation_id <> $1 AND confirmed_at IS NULL AND invalidated_at IS NULL"
        )))
        .bind(id.into_uuid())
        .bind(confirmed_at)
        .execute(&mut *conn)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn invalidate_pending_for_email(
        &mut self,
        email: &Email,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError> {
        invalidate_pending_newsletter_confirmations(self.tx, email, invalidated_at).await
    }

    async fn cleanup_expired(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, NewsletterConfirmationChallengeError> {
        if !(1..=MAX_CLEANUP_BATCH_SIZE).contains(&batch_size) {
            return Err(NewsletterConfirmationChallengeError::InvalidInput);
        }
        Ok(sqlx::query(AssertSqlSafe(format!(
            "WITH expired AS (SELECT confirmation_id FROM {TABLE} WHERE confirmed_at IS NULL AND expires_at <= $1 ORDER BY expires_at ASC, confirmation_id ASC LIMIT $2 FOR UPDATE SKIP LOCKED) DELETE FROM {TABLE} AS challenge USING expired WHERE challenge.confirmation_id = expired.confirmation_id"
        )))
        .bind(now)
        .bind(i64::from(batch_size))
        .execute(self.tx.connection())
        .await
        .map_err(db)?
        .rows_affected())
    }
}

fn send_status_as_str(status: NewsletterConfirmationSendStatus) -> &'static str {
    match status {
        NewsletterConfirmationSendStatus::NotAttempted => "NOT_ATTEMPTED",
        NewsletterConfirmationSendStatus::Accepted => "ACCEPTED",
        NewsletterConfirmationSendStatus::DefinitelyRejected => "DEFINITELY_REJECTED",
        NewsletterConfirmationSendStatus::AcceptanceUnknown => "ACCEPTANCE_UNKNOWN",
    }
}

fn db(error: sqlx::Error) -> NewsletterConfirmationChallengeError {
    NewsletterConfirmationChallengeError::TemporarilyUnavailable {
        source: box_error(error),
    }
}

fn map_consent_persistence_error(
    error: MarketingConsentPersistenceError,
) -> NewsletterConfirmationChallengeError {
    match error {
        MarketingConsentPersistenceError::InvalidPersistedState => {
            NewsletterConfirmationChallengeError::InvalidPersistedState
        }
        MarketingConsentPersistenceError::InvalidInput => {
            NewsletterConfirmationChallengeError::InvalidInput
        }
        MarketingConsentPersistenceError::TemporarilyUnavailable(source) => {
            NewsletterConfirmationChallengeError::TemporarilyUnavailable {
                source: box_error(source),
            }
        }
        MarketingConsentPersistenceError::ConcurrencyConflict
        | MarketingConsentPersistenceError::SourceKeyConflict
        | MarketingConsentPersistenceError::RegisteredEmail => {
            NewsletterConfirmationChallengeError::TemporarilyUnavailable {
                source: box_error(std::io::Error::other(
                    "newsletter confirmation persistence unavailable",
                )),
            }
        }
    }
}
