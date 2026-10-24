use crate::newsletter_confirmation_challenges::invalidate_pending_newsletter_confirmations;
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use serde_email::Email;

use sqlx::{FromRow, PgConnection};
use time::{Duration, OffsetDateTime};
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::user_id::UserId;
use user_service::ports::marketing_consent_intents::{
    ConsentWorkerClaim, ConsentWorkerClaimOutcome, ConsentWorkerFinalization,
    ConsentWorkerRecheckOutcome, ConsentWorkerTerminalStatus, GrantRaceRepairOutcome,
    MarketingConsentIntentWorker,
};
use user_service::ports::{
    ConsentIntent as PortConsentIntent, ConsentIntentSource, ConsentSubject, ConsentUser,
    MarketingConsentIntentError, MarketingConsentIntents, MarketingConsentIntentsFactory,
    NewsletterConfirmationChallengeError, NewsletterProfile, UserStorageVersion,
    marketing_consent_recipient_key,
};
use uuid::Uuid;

const TABLE: &str = "marketing_email_consent_sync_intents";
const COLUMNS: &str = "intent_id, intent_sequence, source_key, subject_type, source, user_id, email, recipient_key, desired, consent_revision, changed_at, profile_snapshot, status, not_after, lease_token, lease_expires_at, attempt_count, completed_lease_token, completed_at, completion_status, last_error_code";
const RETRY_NO_WRITE_PREFIX: &str = "RETRY_NO_WRITE:";

fn is_retry_no_write_marker(value: &str) -> bool {
    value
        .strip_prefix(RETRY_NO_WRITE_PREFIX)
        .is_some_and(|reason| {
            matches!(
                reason,
                "NOT_SENT"
                    | "PROVIDER_REJECTED"
                    | "THROTTLED"
                    | "PREWRITE_READ_UNAVAILABLE"
                    | "PREWRITE_PROTOCOL"
                    | "INVALID_EMAIL"
            )
        })
}

fn bind_source(source: ConsentIntentSource) -> &'static str {
    match source {
        ConsentIntentSource::CognitoSignup => "COGNITO_SIGNUP",
        ConsentIntentSource::AuraDoubleOptIn => "AURA_DOUBLE_OPT_IN",
        ConsentIntentSource::UserWithdrawal => "USER_WITHDRAWAL",
        ConsentIntentSource::UserDeletion => "USER_DELETION",
        ConsentIntentSource::EmailOnlyWithdrawal => "EMAIL_ONLY_WITHDRAWAL",
        ConsentIntentSource::ProviderRaceRepair => "PROVIDER_RACE_REPAIR",
    }
}

fn parse_source(value: &str) -> Result<ConsentIntentSource, MarketingConsentPersistenceError> {
    match value {
        "COGNITO_SIGNUP" => Ok(ConsentIntentSource::CognitoSignup),
        "AURA_DOUBLE_OPT_IN" => Ok(ConsentIntentSource::AuraDoubleOptIn),
        "USER_WITHDRAWAL" => Ok(ConsentIntentSource::UserWithdrawal),
        "USER_DELETION" => Ok(ConsentIntentSource::UserDeletion),
        "EMAIL_ONLY_WITHDRAWAL" => Ok(ConsentIntentSource::EmailOnlyWithdrawal),
        "PROVIDER_RACE_REPAIR" => Ok(ConsentIntentSource::ProviderRaceRepair),
        _ => Err(MarketingConsentPersistenceError::InvalidPersistedState),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsentIntentStatus {
    Pending,
    InProgress,
    Applied,
    Superseded,
    Blocked,
    Failed,
}

impl ConsentIntentStatus {
    fn parse(value: &str) -> Result<Self, MarketingConsentPersistenceError> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "IN_PROGRESS" => Ok(Self::InProgress),
            "APPLIED" => Ok(Self::Applied),
            "SUPERSEDED" => Ok(Self::Superseded),
            "BLOCKED" => Ok(Self::Blocked),
            "FAILED" => Ok(Self::Failed),
            _ => Err(MarketingConsentPersistenceError::InvalidPersistedState),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::InProgress => "IN_PROGRESS",
            Self::Applied => "APPLIED",
            Self::Superseded => "SUPERSEDED",
            Self::Blocked => "BLOCKED",
            Self::Failed => "FAILED",
        }
    }
}

// No Debug: exact email and proof/action key must not enter logs.
#[derive(Clone)]
struct MarketingConsentIntent {
    intent_id: MarketingConsentSyncIntentId,
    source_key: String,
    subject: ConsentSubject,
    source: ConsentIntentSource,
    email: Email,
    profile_snapshot: Option<NewsletterProfile>,
    recipient_key: String,
    desired: bool,
    consent_revision: Option<i64>,
    changed_at: OffsetDateTime,
    status: ConsentIntentStatus,
    not_after: Option<OffsetDateTime>,
}

#[derive(Clone)]
struct MarketingConsentIntentClaim {
    intent: MarketingConsentIntent,
    lease_token: Uuid,
    lease_expires_at: OffsetDateTime,
    attempt_count: i32,
    prior_attempt_write_ambiguous: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MarketingConsentPersistenceError {
    #[error("consent state changed concurrently or user missing")]
    ConcurrencyConflict,
    #[error("source key belongs to a different intent")]
    SourceKeyConflict,
    #[error("email-only intent cannot override registered user consent")]
    RegisteredEmail,
    #[error("invalid consent intent input")]
    InvalidInput,
    #[error("invalid persisted consent intent state")]
    InvalidPersistedState,
    #[error("consent persistence unavailable")]
    TemporarilyUnavailable(#[source] sqlx::Error),
}

fn db(error: sqlx::Error) -> MarketingConsentPersistenceError {
    MarketingConsentPersistenceError::TemporarilyUnavailable(error)
}

fn validate_input(
    source_key: &str,
    changed_at: OffsetDateTime,
    desired: bool,
) -> Result<(), MarketingConsentPersistenceError> {
    if source_key.is_empty()
        || source_key.len() > 512
        || changed_at > OffsetDateTime::now_utc() + Duration::minutes(1)
        || (desired && changed_at + Duration::days(7) <= OffsetDateTime::now_utc())
    {
        Err(MarketingConsentPersistenceError::InvalidInput)
    } else {
        Ok(())
    }
}

// Lock order is source key (on writes), recipient, then rows. The same recipient
// advisory namespace is used for every local and worker transition.
async fn lock_source(
    connection: &mut PgConnection,
    key: &str,
) -> Result<(), MarketingConsentPersistenceError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1936))")
        .bind(key)
        .execute(connection)
        .await
        .map_err(db)?;
    Ok(())
}
pub(crate) async fn lock_recipient(
    connection: &mut PgConnection,
    key: &str,
) -> Result<(), MarketingConsentPersistenceError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1937))")
        .bind(key)
        .execute(connection)
        .await
        .map_err(db)?;
    Ok(())
}

fn encode_profile_snapshot(profile: Option<&NewsletterProfile>) -> Option<serde_json::Value> {
    let profile = profile?;
    let mut snapshot = serde_json::Map::new();
    if let Some(first_name) = &profile.first_name {
        snapshot.insert(
            "first_name".to_owned(),
            serde_json::Value::String(first_name.as_ref().to_owned()),
        );
    }
    if let Some(last_name) = &profile.last_name {
        snapshot.insert(
            "last_name".to_owned(),
            serde_json::Value::String(last_name.as_ref().to_owned()),
        );
    }
    if let Some(language) = profile.language {
        snapshot.insert(
            "language".to_owned(),
            serde_json::Value::String(language.as_str().to_owned()),
        );
    }
    if let Some(currency) = profile.currency {
        snapshot.insert(
            "currency".to_owned(),
            serde_json::Value::String(currency.as_str().to_owned()),
        );
    }
    Some(serde_json::Value::Object(snapshot))
}

fn decode_profile_snapshot(
    snapshot: Option<serde_json::Value>,
) -> Result<Option<NewsletterProfile>, MarketingConsentPersistenceError> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let serde_json::Value::Object(mut fields) = snapshot else {
        return Err(MarketingConsentPersistenceError::InvalidPersistedState);
    };
    let mut take_string = |name: &str| -> Result<Option<String>, MarketingConsentPersistenceError> {
        fields
            .remove(name)
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)
            })
            .transpose()
    };
    let first_name = take_string("first_name")?
        .map(|value| {
            (value.chars().count() <= 64)
                .then(|| user_core::first_name::FirstName::from(value))
                .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)
        })
        .transpose()?;
    let last_name = take_string("last_name")?
        .map(|value| {
            (value.chars().count() <= 64)
                .then(|| user_core::last_name::LastName::from(value))
                .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)
        })
        .transpose()?;
    let language = take_string("language")?
        .map(|value| {
            localization::Language::from_code(&value)
                .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)
        })
        .transpose()?;
    let currency = take_string("currency")?
        .map(|value| {
            money::Currency::from_code(&value)
                .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)
        })
        .transpose()?;
    if !fields.is_empty() {
        return Err(MarketingConsentPersistenceError::InvalidPersistedState);
    }
    Ok(Some(NewsletterProfile {
        first_name,
        last_name,
        language,
        currency,
    }))
}

#[derive(FromRow)]
struct IntentRow {
    intent_id: Uuid,
    intent_sequence: i64,
    source_key: String,
    subject_type: String,
    source: String,
    user_id: Option<Uuid>,
    email: String,
    recipient_key: String,
    desired: bool,
    consent_revision: Option<i64>,
    changed_at: OffsetDateTime,
    profile_snapshot: Option<serde_json::Value>,
    status: String,
    not_after: Option<OffsetDateTime>,
    lease_token: Option<Uuid>,
    lease_expires_at: Option<OffsetDateTime>,
    attempt_count: i32,
    completed_lease_token: Option<Uuid>,
    completed_at: Option<OffsetDateTime>,
    completion_status: Option<String>,
    last_error_code: Option<String>,
}

impl IntentRow {
    fn into_intent(self) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
        let email = Email::try_from(self.email)
            .map_err(|_| MarketingConsentPersistenceError::InvalidPersistedState)?;
        let subject = match (
            self.subject_type.as_str(),
            self.user_id,
            self.consent_revision,
        ) {
            ("USER", Some(id), Some(revision)) if revision > 0 => ConsentSubject::User(
                UserId::try_from(id)
                    .map_err(|_| MarketingConsentPersistenceError::InvalidPersistedState)?,
            ),
            ("EMAIL_ONLY", None, None) => ConsentSubject::EmailOnly,
            _ => return Err(MarketingConsentPersistenceError::InvalidPersistedState),
        };
        let source = parse_source(&self.source)?;
        let status = ConsentIntentStatus::parse(&self.status)?;
        let retry_no_write = self
            .last_error_code
            .as_deref()
            .is_some_and(is_retry_no_write_marker);
        if !source.accepts(subject, self.desired)
            || self.intent_sequence < 1
            || self.attempt_count < 0
            || self.recipient_key != marketing_consent_recipient_key(&email)
            || (self.desired && self.not_after != Some(self.changed_at + Duration::days(7)))
            || (!self.desired && self.not_after.is_some())
            || (status == ConsentIntentStatus::InProgress
                && (self.lease_token.is_none() || self.lease_expires_at.is_none()))
            || (status != ConsentIntentStatus::InProgress
                && (self.lease_token.is_some() || self.lease_expires_at.is_some()))
            || self.completed_lease_token.is_some() != self.completed_at.is_some()
            || self.completed_at.is_some() != self.completion_status.is_some()
            || self
                .last_error_code
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.len() > 128)
            || (retry_no_write
                && (status != ConsentIntentStatus::Pending || self.attempt_count == 0))
            || (status == ConsentIntentStatus::Pending
                && (self.attempt_count > 0) != retry_no_write)
            || self.completion_status.as_deref().is_some_and(|value| {
                value != self.status
                    || !matches!(
                        status,
                        ConsentIntentStatus::Applied
                            | ConsentIntentStatus::Superseded
                            | ConsentIntentStatus::Blocked
                            | ConsentIntentStatus::Failed
                    )
            })
        {
            return Err(MarketingConsentPersistenceError::InvalidPersistedState);
        }
        Ok(MarketingConsentIntent {
            intent_id: MarketingConsentSyncIntentId::try_from(self.intent_id)
                .map_err(|_| MarketingConsentPersistenceError::InvalidPersistedState)?,
            source_key: self.source_key,
            subject,
            source,
            email,
            recipient_key: self.recipient_key,
            profile_snapshot: decode_profile_snapshot(self.profile_snapshot)?,
            desired: self.desired,
            consent_revision: self.consent_revision,
            changed_at: self.changed_at,
            status,
            not_after: self.not_after,
        })
    }
}

async fn find_source(
    connection: &mut PgConnection,
    key: &str,
) -> Result<Option<MarketingConsentIntent>, MarketingConsentPersistenceError> {
    let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE source_key = $1");
    sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
        .bind(key)
        .fetch_optional(connection)
        .await
        .map_err(db)?
        .map(IntentRow::into_intent)
        .transpose()
}

#[expect(
    clippy::too_many_arguments,
    reason = "an intent records all immutable decision fields"
)]
async fn append(
    connection: &mut PgConnection,
    subject: ConsentSubject,
    source: ConsentIntentSource,
    email: &Email,
    desired: bool,
    revision: Option<i64>,
    source_key: &str,
    changed_at: OffsetDateTime,
    profile_snapshot: Option<&NewsletterProfile>,
) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
    let key = marketing_consent_recipient_key(email);
    let (kind, id) = match subject {
        ConsentSubject::User(id) => ("USER", Some(*id.as_uuid())),
        ConsentSubject::EmailOnly => ("EMAIL_ONLY", None),
    };
    let sql = format!(
        "INSERT INTO {TABLE} (intent_id, source_key, subject_type, source, user_id, email, recipient_key, desired, consent_revision, changed_at, profile_snapshot, not_after) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CASE WHEN $8 THEN $10::timestamptz + interval '7 days' END) RETURNING {COLUMNS}"
    );
    let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
        .bind(MarketingConsentSyncIntentId::new().into_uuid())
        .bind(source_key)
        .bind(kind)
        .bind(bind_source(source))
        .bind(id)
        .bind::<&str>(email.as_ref())
        .bind(&key)
        .bind(desired)
        .bind(revision)
        .bind(changed_at)
        .bind(encode_profile_snapshot(profile_snapshot))
        .fetch_one(&mut *connection)
        .await
        .map_err(db)?;
    let sequence = row.intent_sequence;
    let intent = row.into_intent()?;
    // A later decision invalidates earlier unsent or leased grants. Terminal
    // history and revokes are retained, never changed into permission to send.
    let sql = format!(
        "UPDATE {TABLE} SET status = 'SUPERSEDED', last_error_code = NULL, lease_token = NULL, lease_expires_at = NULL, updated = clock_timestamp() WHERE recipient_key = $1 AND email = $3 AND intent_sequence < $2 AND desired AND status IN ('PENDING', 'IN_PROGRESS')"
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(&key)
        .bind(sequence)
        .bind::<&str>(email.as_ref())
        .execute(connection)
        .await
        .map_err(db)?;
    Ok(intent)
}

#[derive(Default, Clone, Copy)]
pub struct SqlxMarketingConsentIntentRepository;
impl SqlxMarketingConsentIntentRepository {
    pub fn new() -> Self {
        Self
    }
    #[cfg(test)]
    async fn find_by_source_key(
        &self,
        tx: &mut SqlxTransaction,
        source_key: &str,
    ) -> Result<Option<MarketingConsentIntent>, MarketingConsentPersistenceError> {
        find_source(tx.connection(), source_key).await
    }

    /// The verified address is checked against the exact canonical account email.
    /// Distinct accepted proofs advance the revision even if consent was already true.
    #[expect(
        clippy::too_many_arguments,
        reason = "the transaction-bound write requires the exact proof and user fence"
    )]
    #[cfg(test)]
    async fn record_user_transition(
        &self,
        tx: &mut SqlxTransaction,
        user_id: UserId,
        expected_version: UserStorageVersion,
        verified_email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
        self.record_user_transition_with_profile(
            tx,
            user_id,
            expected_version,
            verified_email,
            desired,
            source,
            source_key,
            None,
            changed_at,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the transaction-bound write requires the exact proof and user fence"
    )]
    async fn record_user_transition_with_profile(
        &self,
        tx: &mut SqlxTransaction,
        user_id: UserId,
        expected_version: UserStorageVersion,
        verified_email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
        validate_input(source_key, changed_at, desired)?;
        if !source.accepts(ConsentSubject::User(user_id), desired)
            || matches!(
                source,
                ConsentIntentSource::UserDeletion | ConsentIntentSource::ProviderRaceRepair
            )
        {
            return Err(MarketingConsentPersistenceError::InvalidInput);
        }
        let conn = tx.connection();
        lock_source(&mut *conn, source_key).await?;
        if let Some(existing) = find_source(&mut *conn, source_key).await? {
            return if existing.subject == ConsentSubject::User(user_id)
                && existing.email == *verified_email
                && existing.desired == desired
                && existing.source == source
                && existing.profile_snapshot == profile_snapshot
            {
                Ok(existing)
            } else {
                Err(MarketingConsentPersistenceError::SourceKeyConflict)
            };
        }
        let key = marketing_consent_recipient_key(verified_email);
        lock_recipient(&mut *conn, &key).await?;
        let version = i64::try_from(expected_version.into_inner())
            .map_err(|_| MarketingConsentPersistenceError::InvalidInput)?;
        let revision: i64 = sqlx::query_scalar("UPDATE users SET marketing_email_consent = $2, marketing_email_consent_revision = marketing_email_consent_revision + 1, marketing_email_consent_changed_at = $3, version = version + 1, updated = aura_advance_updated(updated) WHERE user_id = $1 AND email = $4 AND version = $5 RETURNING marketing_email_consent_revision")
            .bind(user_id.as_uuid()).bind(desired).bind(changed_at).bind::<&str>(verified_email.as_ref()).bind(version)
            .fetch_optional(&mut *conn).await.map_err(db)?.ok_or(MarketingConsentPersistenceError::ConcurrencyConflict)?;
        append(
            conn,
            ConsentSubject::User(user_id),
            source,
            verified_email,
            desired,
            Some(revision),
            source_key,
            changed_at,
            profile_snapshot.as_ref(),
        )
        .await
    }

    #[cfg(test)]
    async fn record_email_only_intent(
        &self,
        tx: &mut SqlxTransaction,
        email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
        self.record_email_only_intent_with_profile(
            tx, email, desired, source, source_key, None, changed_at,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the transaction-bound write records the exact email-only decision"
    )]
    async fn record_email_only_intent_with_profile(
        &self,
        tx: &mut SqlxTransaction,
        email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
        validate_input(source_key, changed_at, desired)?;
        if !source.accepts(ConsentSubject::EmailOnly, desired)
            || source == ConsentIntentSource::ProviderRaceRepair
        {
            return Err(MarketingConsentPersistenceError::InvalidInput);
        }
        let conn = tx.connection();
        lock_source(&mut *conn, source_key).await?;
        if let Some(existing) = find_source(&mut *conn, source_key).await? {
            return if existing.subject == ConsentSubject::EmailOnly
                && existing.email == *email
                && existing.desired == desired
                && existing.source == source
                && existing.profile_snapshot == profile_snapshot
            {
                Ok(existing)
            } else {
                Err(MarketingConsentPersistenceError::SourceKeyConflict)
            };
        }
        lock_recipient(&mut *conn, &marketing_consent_recipient_key(email)).await?;
        let registered: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
                .bind::<&str>(email.as_ref())
                .fetch_one(&mut *conn)
                .await
                .map_err(db)?;
        if registered {
            return Err(MarketingConsentPersistenceError::RegisteredEmail);
        }
        append(
            conn,
            ConsentSubject::EmailOnly,
            source,
            email,
            desired,
            None,
            source_key,
            changed_at,
            profile_snapshot.as_ref(),
        )
        .await
    }

    /// Back-sync is inbound state: cancel unsent local grants, never enqueue an echo.
    async fn cancel_provider_backsync(
        &self,
        tx: &mut SqlxTransaction,
        email: &Email,
    ) -> Result<u64, MarketingConsentPersistenceError> {
        let conn = tx.connection();
        let key = marketing_consent_recipient_key(email);
        lock_recipient(&mut *conn, &key).await?;
        let sql = format!(
            "UPDATE {TABLE} SET status = 'BLOCKED', last_error_code = CASE WHEN status = 'IN_PROGRESS' THEN 'PROVIDER_WITHDRAWAL_RACE_CANDIDATE' ELSE 'PROVIDER_WITHDRAWAL_UNSENT' END, lease_token = NULL, lease_expires_at = NULL, updated = clock_timestamp() WHERE recipient_key = $1 AND email = $2 AND desired AND status IN ('PENDING', 'IN_PROGRESS')"
        );
        Ok(sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(key)
            .bind::<&str>(email.as_ref())
            .execute(conn)
            .await
            .map_err(db)?
            .rows_affected())
    }

    /// Only a leased grant canceled by provider back-sync is a compensation candidate.
    /// Serialize on the recipient alone, so this is safe after a worker recheck
    /// in the same transaction. The source-key uniqueness remains a final invariant.
    async fn repair_raced_grant_if_needed(
        &self,
        tx: &mut SqlxTransaction,
        original: MarketingConsentSyncIntentId,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<GrantRaceRepairOutcome, MarketingConsentPersistenceError> {
        validate_input(source_key, changed_at, false)?;
        let conn = tx.connection();
        let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1");
        let Some(initial) = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
            .bind(original.as_uuid())
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?
        else {
            return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
        };
        let key = initial.recipient_key.clone();
        initial.into_intent()?;
        lock_recipient(&mut *conn, &key).await?;

        let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1 FOR UPDATE"
        )))
        .bind(original.as_uuid())
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        if row.recipient_key != key {
            return Err(MarketingConsentPersistenceError::InvalidPersistedState);
        }
        let sequence = row.intent_sequence;
        let attempts = row.attempt_count;
        let finalized = row.completed_lease_token.is_some();
        let provider_race = row.status == "BLOCKED"
            && row.last_error_code.as_deref() == Some("PROVIDER_WITHDRAWAL_RACE_CANDIDATE");
        let intent = row.into_intent()?;
        if !intent.desired || attempts == 0 || finalized {
            return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
        }
        if !provider_race && intent.status != ConsentIntentStatus::Superseded {
            return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
        }
        // A later account assignment makes even an old local revoke unsafe to
        // advertise as corrective work for this immutable target.
        let current_owner: Option<Uuid> =
            sqlx::query_scalar("SELECT user_id FROM users WHERE email = $1")
                .bind::<&str>(intent.email.as_ref())
                .fetch_optional(&mut *conn)
                .await
                .map_err(db)?;
        match intent.subject {
            ConsentSubject::User(id)
                if current_owner.is_some_and(|owner| owner != *id.as_uuid()) =>
            {
                return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
            }
            ConsentSubject::EmailOnly if current_owner.is_some() => {
                return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
            }
            _ => {}
        }
        let newer = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM {TABLE} WHERE recipient_key = $1 AND email = $2 AND intent_sequence > $3 ORDER BY intent_sequence DESC LIMIT 1"
        )))
        .bind(&key)
        .bind::<&str>(intent.email.as_ref())
        .bind(sequence)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db)?;
        if let Some(newer) = newer {
            let newer = newer.into_intent()?;
            return Ok(if !newer.desired {
                GrantRaceRepairOutcome::ExistingRepair(newer.intent_id)
            } else {
                GrantRaceRepairOutcome::NoRepairNeeded
            });
        }
        if !provider_race {
            return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
        }
        let revision = match intent.subject {
            ConsentSubject::User(id) => {
                let state: Option<(String, bool, i64)> = sqlx::query_as(
                    "SELECT email, marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1",
                )
                .bind(id.as_uuid())
                .fetch_optional(&mut *conn)
                .await
                .map_err(db)?;
                match state {
                    Some((email, false, revision))
                        if email == <Email as AsRef<str>>::as_ref(&intent.email)
                            && Some(revision) > intent.consent_revision =>
                    {
                        Some(revision)
                    }
                    _ => return Ok(GrantRaceRepairOutcome::NoRepairNeeded),
                }
            }
            ConsentSubject::EmailOnly => {
                let registered: bool =
                    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
                        .bind::<&str>(intent.email.as_ref())
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(db)?;
                if registered {
                    return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
                }
                None
            }
        };
        let repair = append(
            conn,
            intent.subject,
            ConsentIntentSource::ProviderRaceRepair,
            &intent.email,
            false,
            revision,
            source_key,
            changed_at,
            None,
        )
        .await?;
        Ok(GrantRaceRepairOutcome::RepairScheduled(repair.intent_id))
    }

    /// Delete the account but retain a USER revoke addressed to the old email.
    async fn record_user_deletion(
        &self,
        tx: &mut SqlxTransaction,
        user_id: UserId,
        expected_version: UserStorageVersion,
        old_email: &Email,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentIntent, MarketingConsentPersistenceError> {
        validate_input(source_key, changed_at, false)?;
        let conn = tx.connection();
        lock_source(&mut *conn, source_key).await?;
        if let Some(existing) = find_source(&mut *conn, source_key).await? {
            return if existing.subject == ConsentSubject::User(user_id)
                && existing.email == *old_email
                && existing.source == ConsentIntentSource::UserDeletion
            {
                Ok(existing)
            } else {
                Err(MarketingConsentPersistenceError::SourceKeyConflict)
            };
        }
        lock_recipient(&mut *conn, &marketing_consent_recipient_key(old_email)).await?;
        let version = i64::try_from(expected_version.into_inner())
            .map_err(|_| MarketingConsentPersistenceError::InvalidInput)?;
        let revision: i64 = sqlx::query_scalar("DELETE FROM users WHERE user_id = $1 AND email = $2 AND version = $3 RETURNING marketing_email_consent_revision + 1")
            .bind(user_id.as_uuid()).bind::<&str>(old_email.as_ref()).bind(version)
            .fetch_optional(&mut *conn).await.map_err(db)?.ok_or(MarketingConsentPersistenceError::ConcurrencyConflict)?;
        append(
            conn,
            ConsentSubject::User(user_id),
            ConsentIntentSource::UserDeletion,
            old_email,
            false,
            Some(revision),
            source_key,
            changed_at,
            None,
        )
        .await
    }
}

struct SqlxMarketingConsentIntents<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl MarketingConsentIntentsFactory<SqlxTransaction> for SqlxMarketingConsentIntentRepository {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl MarketingConsentIntents + 'tx {
        SqlxMarketingConsentIntents { tx }
    }
}

impl From<MarketingConsentPersistenceError> for MarketingConsentIntentError {
    fn from(error: MarketingConsentPersistenceError) -> Self {
        match error {
            MarketingConsentPersistenceError::ConcurrencyConflict => Self::ConcurrencyConflict,
            MarketingConsentPersistenceError::SourceKeyConflict => Self::SourceKeyConflict,
            MarketingConsentPersistenceError::RegisteredEmail => Self::RegisteredEmail,
            MarketingConsentPersistenceError::InvalidInput => Self::InvalidInput,
            MarketingConsentPersistenceError::InvalidPersistedState => Self::InvalidPersistedState,
            MarketingConsentPersistenceError::TemporarilyUnavailable(source) => {
                Self::TemporarilyUnavailable {
                    source: box_error(source),
                }
            }
        }
    }
}

impl From<MarketingConsentIntent> for PortConsentIntent {
    fn from(intent: MarketingConsentIntent) -> Self {
        Self {
            intent_id: intent.intent_id,
            source_key: intent.source_key,
            subject: intent.subject,
            source: intent.source,
            email: intent.email,
            profile_snapshot: intent.profile_snapshot.map(Box::new),
            desired: intent.desired,
        }
    }
}

#[derive(FromRow)]
struct ConsentUserRow {
    user_id: Uuid,
    email: String,
    version: i64,
    marketing_email_consent: bool,
    marketing_email_consent_revision: i64,
    marketing_email_consent_changed_at: Option<OffsetDateTime>,
}
impl TryFrom<ConsentUserRow> for ConsentUser {
    type Error = MarketingConsentPersistenceError;
    fn try_from(row: ConsentUserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::try_from(row.user_id)
                .map_err(|_| Self::Error::InvalidPersistedState)?,
            email: Email::try_from(row.email).map_err(|_| Self::Error::InvalidPersistedState)?,
            version: UserStorageVersion::try_from(row.version)
                .map_err(|_| Self::Error::InvalidPersistedState)?,
            marketing_email_consent: row.marketing_email_consent,
            marketing_email_consent_revision: row.marketing_email_consent_revision,
            marketing_email_consent_changed_at: row.marketing_email_consent_changed_at,
        })
    }
}

#[async_trait::async_trait]
impl MarketingConsentIntents for SqlxMarketingConsentIntents<'_> {
    async fn lock_recipient(&mut self, email: &Email) -> Result<(), MarketingConsentIntentError> {
        lock_recipient(
            self.tx.connection(),
            &marketing_consent_recipient_key(email),
        )
        .await?;
        Ok(())
    }

    async fn lock_source_key(&mut self, key: &str) -> Result<(), MarketingConsentIntentError> {
        lock_source(self.tx.connection(), key).await?;
        Ok(())
    }

    async fn find_by_source_key(
        &mut self,
        key: &str,
    ) -> Result<Option<PortConsentIntent>, MarketingConsentIntentError> {
        Ok(find_source(self.tx.connection(), key)
            .await?
            .map(Into::into))
    }

    async fn find_user_by_id(
        &mut self,
        id: UserId,
    ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
        sqlx::query_as::<_, ConsentUserRow>(
            "SELECT user_id, email, version, marketing_email_consent, marketing_email_consent_revision, marketing_email_consent_changed_at FROM users WHERE user_id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(self.tx.connection())
        .await
        .map_err(db)?
        .map(ConsentUser::try_from)
        .transpose()
        .map_err(Into::into)
    }

    async fn find_user_by_email(
        &mut self,
        email: &Email,
    ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
        sqlx::query_as::<_, ConsentUserRow>(
            "SELECT user_id, email, version, marketing_email_consent, marketing_email_consent_revision, marketing_email_consent_changed_at FROM users WHERE email = $1",
        )
        .bind::<&str>(email.as_ref())
        .fetch_optional(self.tx.connection())
        .await
        .map_err(db)?
        .map(ConsentUser::try_from)
        .transpose()
        .map_err(Into::into)
    }

    async fn record_user_transition(
        &mut self,
        user: &ConsentUser,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<PortConsentIntent, MarketingConsentIntentError> {
        Ok(SqlxMarketingConsentIntentRepository::new()
            .record_user_transition_with_profile(
                self.tx,
                user.user_id,
                user.version,
                &user.email,
                desired,
                source,
                source_key,
                profile_snapshot,
                changed_at,
            )
            .await?
            .into())
    }

    async fn record_email_only_intent(
        &mut self,
        email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<PortConsentIntent, MarketingConsentIntentError> {
        Ok(SqlxMarketingConsentIntentRepository::new()
            .record_email_only_intent_with_profile(
                self.tx,
                email,
                desired,
                source,
                source_key,
                profile_snapshot,
                changed_at,
            )
            .await?
            .into())
    }

    async fn apply_provider_withdrawal(
        &mut self,
        user: &ConsentUser,
        changed_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError> {
        if changed_at > OffsetDateTime::now_utc() + Duration::minutes(1) {
            return Err(MarketingConsentIntentError::InvalidInput);
        }
        let conn = self.tx.connection();
        lock_recipient(&mut *conn, &marketing_consent_recipient_key(&user.email)).await?;
        let version = i64::try_from(user.version.into_inner())
            .map_err(|_| MarketingConsentIntentError::InvalidInput)?;
        let updated = sqlx::query("UPDATE users SET marketing_email_consent = false, marketing_email_consent_revision = marketing_email_consent_revision + 1, marketing_email_consent_changed_at = $3, version = version + 1, updated = aura_advance_updated(updated) WHERE user_id = $1 AND email = $2 AND version = $4 AND marketing_email_consent = true")
            .bind(user.user_id.as_uuid()).bind::<&str>(user.email.as_ref()).bind(changed_at).bind(version)
            .execute(&mut *conn).await.map_err(db)?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }
        let current: Option<(bool,)> = sqlx::query_as("SELECT marketing_email_consent FROM users WHERE user_id = $1 AND email = $2 AND version = $3")
            .bind(user.user_id.as_uuid()).bind::<&str>(user.email.as_ref()).bind(version)
            .fetch_optional(&mut *conn).await.map_err(db)?;
        match current {
            Some((false,)) => Ok(()),
            _ => Err(MarketingConsentIntentError::ConcurrencyConflict),
        }
    }

    async fn apply_provider_resubscription(
        &mut self,
        user: &ConsentUser,
        changed_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError> {
        if changed_at > OffsetDateTime::now_utc() + Duration::minutes(1) {
            return Err(MarketingConsentIntentError::InvalidInput);
        }
        let conn = self.tx.connection();
        lock_recipient(&mut *conn, &marketing_consent_recipient_key(&user.email)).await?;
        let version = i64::try_from(user.version.into_inner())
            .map_err(|_| MarketingConsentIntentError::InvalidInput)?;
        let updated = sqlx::query("UPDATE users SET marketing_email_consent = true, marketing_email_consent_revision = marketing_email_consent_revision + 1, marketing_email_consent_changed_at = $3, version = version + 1, updated = aura_advance_updated(updated) WHERE user_id = $1 AND email = $2 AND version = $4 AND marketing_email_consent = false")
            .bind(user.user_id.as_uuid()).bind::<&str>(user.email.as_ref()).bind(changed_at).bind(version)
            .execute(&mut *conn).await.map_err(db)?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }
        let current: Option<(bool,)> = sqlx::query_as("SELECT marketing_email_consent FROM users WHERE user_id = $1 AND email = $2 AND version = $3")
            .bind(user.user_id.as_uuid()).bind::<&str>(user.email.as_ref()).bind(version)
            .fetch_optional(&mut *conn).await.map_err(db)?;
        match current {
            Some((true,)) => Ok(()),
            _ => Err(MarketingConsentIntentError::ConcurrencyConflict),
        }
    }

    async fn cancel_provider_backsync(
        &mut self,
        email: &Email,
    ) -> Result<(), MarketingConsentIntentError> {
        SqlxMarketingConsentIntentRepository::new()
            .cancel_provider_backsync(self.tx, email)
            .await?;
        Ok(())
    }

    async fn invalidate_newsletter_confirmation_challenges(
        &mut self,
        email: &Email,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError> {
        invalidate_pending_newsletter_confirmations(self.tx, email, invalidated_at)
            .await
            .map_err(|error| match error {
                NewsletterConfirmationChallengeError::InvalidPersistedState => {
                    MarketingConsentIntentError::InvalidPersistedState
                }
                NewsletterConfirmationChallengeError::InvalidInput => {
                    MarketingConsentIntentError::InvalidInput
                }
                NewsletterConfirmationChallengeError::TemporarilyUnavailable { source } => {
                    MarketingConsentIntentError::TemporarilyUnavailable { source }
                }
            })
    }

    async fn repair_raced_grant_if_needed(
        &mut self,
        original: MarketingConsentSyncIntentId,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<GrantRaceRepairOutcome, MarketingConsentIntentError> {
        Ok(SqlxMarketingConsentIntentRepository::new()
            .repair_raced_grant_if_needed(self.tx, original, source_key, changed_at)
            .await?)
    }

    async fn record_user_deletion(
        &mut self,
        user: &ConsentUser,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<PortConsentIntent, MarketingConsentIntentError> {
        Ok(SqlxMarketingConsentIntentRepository::new()
            .record_user_deletion(
                self.tx,
                user.user_id,
                user.version,
                &user.email,
                source_key,
                changed_at,
            )
            .await?
            .into())
    }
}

#[derive(Clone, Copy, Default)]
pub struct SqlxMarketingConsentIntentWorker;
impl SqlxMarketingConsentIntentWorker {
    pub fn new() -> Self {
        Self
    }

    /// Claim only the requested durable ID; a busy or terminal row never makes
    /// the worker claim some unrelated recipient's intent.
    async fn claim_by_id(
        &self,
        tx: &mut SqlxTransaction,
        id: MarketingConsentSyncIntentId,
    ) -> Result<ConsentWorkerClaimOutcome, MarketingConsentPersistenceError> {
        let conn = tx.connection();
        let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1");
        let Some(candidate) = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
            .bind(id.as_uuid())
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?
        else {
            return Ok(ConsentWorkerClaimOutcome::Missing);
        };
        let key = candidate.recipient_key.clone();
        candidate.into_intent()?;
        lock_recipient(conn, &key).await?;
        let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1 FOR UPDATE");
        let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
            .bind(id.as_uuid())
            .fetch_optional(tx.connection())
            .await
            .map_err(db)?;
        let Some(row) = row else {
            return Ok(ConsentWorkerClaimOutcome::Missing);
        };
        if row.recipient_key != key {
            return Err(MarketingConsentPersistenceError::InvalidPersistedState);
        }
        let status = ConsentIntentStatus::parse(&row.status)?;
        let expires = row.lease_expires_at;
        let prior_attempt_write_ambiguous = status == ConsentIntentStatus::InProgress;
        row.into_intent()?;
        match status {
            ConsentIntentStatus::Applied => {
                return Ok(ConsentWorkerClaimOutcome::Terminal(
                    ConsentWorkerTerminalStatus::Applied,
                ));
            }
            ConsentIntentStatus::Superseded => {
                return Ok(ConsentWorkerClaimOutcome::Terminal(
                    ConsentWorkerTerminalStatus::Superseded,
                ));
            }
            ConsentIntentStatus::Blocked => {
                return Ok(ConsentWorkerClaimOutcome::Terminal(
                    ConsentWorkerTerminalStatus::Blocked,
                ));
            }
            ConsentIntentStatus::Failed => {
                return Ok(ConsentWorkerClaimOutcome::Terminal(
                    ConsentWorkerTerminalStatus::Failed,
                ));
            }
            ConsentIntentStatus::InProgress
                if expires.is_some_and(|at| at > OffsetDateTime::now_utc()) =>
            {
                return Ok(ConsentWorkerClaimOutcome::Deferred {
                    lease_expires_at: expires.expect("checked above"),
                });
            }
            _ => {}
        }
        let sql = format!(
            "UPDATE {TABLE} SET status = 'IN_PROGRESS', lease_token = $2, lease_expires_at = clock_timestamp() + interval '5 minutes', completed_lease_token = NULL, completed_at = NULL, completion_status = NULL, last_error_code = NULL, provider_contact_id = NULL, attempt_count = attempt_count + 1, updated = clock_timestamp() WHERE intent_id = $1 AND (status = 'PENDING' OR (status = 'IN_PROGRESS' AND lease_expires_at <= clock_timestamp())) RETURNING {COLUMNS}"
        );
        let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
            .bind(id.as_uuid())
            .bind(Uuid::now_v7())
            .fetch_optional(tx.connection())
            .await
            .map_err(db)?;
        let Some(row) = row else {
            // The server clock may disagree with the caller near expiry. Report
            // the persisted lease rather than inventing custody of this row.
            let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1");
            let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
                .bind(id.as_uuid())
                .fetch_one(tx.connection())
                .await
                .map_err(db)?;
            let status = ConsentIntentStatus::parse(&row.status)?;
            let expires = row.lease_expires_at;
            row.into_intent()?;
            return match status {
                ConsentIntentStatus::InProgress => Ok(ConsentWorkerClaimOutcome::Deferred {
                    lease_expires_at: expires
                        .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)?,
                }),
                _ => Err(MarketingConsentPersistenceError::InvalidPersistedState),
            };
        };
        let claim = MarketingConsentIntentClaim {
            lease_token: row
                .lease_token
                .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)?,
            lease_expires_at: row
                .lease_expires_at
                .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)?,
            attempt_count: row.attempt_count,
            prior_attempt_write_ambiguous,
            intent: row.into_intent()?,
        };
        if self.read_claim(tx, &claim).await?.is_none() {
            let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1");
            let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
                .bind(id.as_uuid())
                .fetch_one(tx.connection())
                .await
                .map_err(db)?;
            let status = ConsentIntentStatus::parse(&row.status)?;
            let expires = row.lease_expires_at;
            row.into_intent()?;
            return Ok(match status {
                ConsentIntentStatus::Superseded => {
                    ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Superseded)
                }
                ConsentIntentStatus::Blocked => {
                    ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
                }
                ConsentIntentStatus::InProgress => ConsentWorkerClaimOutcome::Deferred {
                    lease_expires_at: expires
                        .ok_or(MarketingConsentPersistenceError::InvalidPersistedState)?,
                },
                _ => return Err(MarketingConsentPersistenceError::InvalidPersistedState),
            });
        }
        Ok(ConsentWorkerClaimOutcome::Claimed(claim.into()))
    }

    /// Recheck the authoritative decision immediately before provider I/O.
    async fn read_claim(
        &self,
        tx: &mut SqlxTransaction,
        claim: &MarketingConsentIntentClaim,
    ) -> Result<Option<MarketingConsentIntent>, MarketingConsentPersistenceError> {
        let conn = tx.connection();
        lock_recipient(&mut *conn, &claim.intent.recipient_key).await?;
        let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1 FOR UPDATE");
        let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
            .bind(claim.intent.intent_id.as_uuid())
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.recipient_key != claim.intent.recipient_key
            || row.email != claim.intent.email.as_ref() as &str
            || row.desired != claim.intent.desired
            || row.status != "IN_PROGRESS"
            || row.lease_token != Some(claim.lease_token)
        {
            return Ok(None);
        }
        if row
            .lease_expires_at
            .is_none_or(|at| at <= OffsetDateTime::now_utc())
        {
            return Ok(None);
        }
        let intent = row.into_intent()?;
        if intent.subject != claim.intent.subject
            || intent.source != claim.intent.source
            || intent.consent_revision != claim.intent.consent_revision
            || intent.source_key != claim.intent.source_key
            || intent.changed_at != claim.intent.changed_at
            || intent.not_after != claim.intent.not_after
        {
            return Ok(None);
        }
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {TABLE} WHERE recipient_key = $1 AND email = $3 AND intent_sequence > (SELECT intent_sequence FROM {TABLE} WHERE intent_id = $2))"
        );
        let newer: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(&intent.recipient_key)
            .bind(intent.intent_id.as_uuid())
            .bind::<&str>(intent.email.as_ref())
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        let status = if newer {
            Some(ConsentIntentStatus::Superseded)
        } else if intent.desired
            && intent
                .not_after
                .is_some_and(|at| at <= OffsetDateTime::now_utc())
        {
            Some(ConsentIntentStatus::Blocked)
        } else {
            match intent.subject {
                ConsentSubject::User(id) => {
                    let state: Option<(String, bool, i64)> = sqlx::query_as("SELECT email, marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1")
                        .bind(id.as_uuid()).fetch_optional(&mut *conn).await.map_err(db)?;
                    let decision = match (intent.source, state) {
                        (ConsentIntentSource::UserDeletion, None) if !intent.desired => None,
                        (_, Some((email, consent, revision)))
                            if email == <Email as AsRef<str>>::as_ref(&intent.email)
                                && consent == intent.desired
                                && Some(revision) == intent.consent_revision =>
                        {
                            None
                        }
                        _ => Some(ConsentIntentStatus::Superseded),
                    };
                    if decision.is_none() && intent.source == ConsentIntentSource::UserDeletion {
                        // The old address may have been assigned to a different account.
                        // A retained revoke cannot withdraw that new owner's consent.
                        let different_owner: bool = sqlx::query_scalar(
                            "SELECT EXISTS (SELECT 1 FROM users WHERE email = $1 AND user_id <> $2)",
                        )
                        .bind::<&str>(intent.email.as_ref())
                        .bind(id.as_uuid())
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(db)?;
                        if different_owner {
                            Some(ConsentIntentStatus::Blocked)
                        } else {
                            None
                        }
                    } else {
                        decision
                    }
                }
                ConsentSubject::EmailOnly => {
                    let registered: bool =
                        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
                            .bind::<&str>(intent.email.as_ref())
                            .fetch_one(&mut *conn)
                            .await
                            .map_err(db)?;
                    registered.then_some(ConsentIntentStatus::Blocked)
                }
            }
        };
        if let Some(status) = status {
            let sql = format!(
                "UPDATE {TABLE} SET status = $2, last_error_code = CASE WHEN $2 = 'BLOCKED' AND not_after <= clock_timestamp() THEN 'GRANT_EXPIRED' WHEN $2 = 'BLOCKED' AND subject_type = 'EMAIL_ONLY' THEN 'REGISTERED_ADDRESS' WHEN $2 = 'BLOCKED' AND source = 'USER_DELETION' THEN 'ADDRESS_REASSIGNED' ELSE last_error_code END, lease_token = NULL, lease_expires_at = NULL, updated = clock_timestamp() WHERE intent_id = $1"
            );
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(intent.intent_id.as_uuid())
                .bind(status.as_str())
                .execute(conn)
                .await
                .map_err(db)?;
            return Ok(None);
        }
        Ok(Some(intent))
    }

    async fn finalize(
        &self,
        tx: &mut SqlxTransaction,
        claim: &MarketingConsentIntentClaim,
        result: ConsentWorkerFinalization<'_>,
        completed_at: OffsetDateTime,
    ) -> Result<bool, MarketingConsentPersistenceError> {
        let (status, contact_id, error_code) = match result {
            ConsentWorkerFinalization::Applied {
                provider_contact_id,
            } => (ConsentIntentStatus::Applied, provider_contact_id, None),
            ConsentWorkerFinalization::Failed { error_code } => {
                (ConsentIntentStatus::Failed, None, Some(error_code))
            }
            ConsentWorkerFinalization::Blocked { error_code } => {
                (ConsentIntentStatus::Blocked, None, Some(error_code))
            }
        };
        if error_code.is_some_and(|code| code.is_empty() || code.len() > 128)
            || contact_id.is_some_and(|id| id.is_empty() || id.len() > 512)
        {
            return Err(MarketingConsentPersistenceError::InvalidInput);
        }
        let conn = tx.connection();
        lock_recipient(&mut *conn, &claim.intent.recipient_key).await?;
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {TABLE} WHERE intent_id = $1 AND completed_lease_token = $2 AND status = $3 AND completed_at = $4 AND provider_contact_id IS NOT DISTINCT FROM $5 AND last_error_code IS NOT DISTINCT FROM $6 AND email = $7 AND recipient_key = $8 AND desired = $9 AND source = $10 AND user_id IS NOT DISTINCT FROM $11 AND source_key = $12 AND consent_revision IS NOT DISTINCT FROM $13 AND changed_at = $14 AND not_after IS NOT DISTINCT FROM $15)"
        );
        let replay: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(claim.intent.intent_id.as_uuid())
            .bind(claim.lease_token)
            .bind(status.as_str())
            .bind(completed_at)
            .bind(contact_id)
            .bind(error_code)
            .bind::<&str>(claim.intent.email.as_ref())
            .bind(&claim.intent.recipient_key)
            .bind(claim.intent.desired)
            .bind(bind_source(claim.intent.source))
            .bind(match claim.intent.subject {
                ConsentSubject::User(id) => Some(*id.as_uuid()),
                ConsentSubject::EmailOnly => None,
            })
            .bind(&claim.intent.source_key)
            .bind(claim.intent.consent_revision)
            .bind(claim.intent.changed_at)
            .bind(claim.intent.not_after)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        if replay {
            return Ok(true);
        }
        if self.read_claim(tx, claim).await?.is_none() {
            return Ok(false);
        }
        let sql = format!(
            "UPDATE {TABLE} SET status = $3, lease_token = NULL, lease_expires_at = NULL, completed_lease_token = $2, completed_at = $4, completion_status = $3, provider_contact_id = $5, last_error_code = $6, updated = clock_timestamp() WHERE intent_id = $1 AND status = 'IN_PROGRESS' AND lease_token = $2 AND lease_expires_at > clock_timestamp() AND lease_expires_at > $4 AND (NOT desired OR not_after > clock_timestamp())"
        );
        let updated = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(claim.intent.intent_id.as_uuid())
            .bind(claim.lease_token)
            .bind(status.as_str())
            .bind(completed_at)
            .bind(contact_id)
            .bind(error_code)
            .execute(tx.connection())
            .await
            .map_err(db)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Release only a known non-writing attempt. The durable marker lets the next exact-ID
    /// claim distinguish this retry from an abandoned lease with an ambiguous provider write.
    async fn release_for_retry(
        &self,
        tx: &mut SqlxTransaction,
        claim: &MarketingConsentIntentClaim,
        reason_code: &str,
    ) -> Result<bool, MarketingConsentPersistenceError> {
        if !matches!(
            reason_code,
            "NOT_SENT"
                | "PROVIDER_REJECTED"
                | "THROTTLED"
                | "PREWRITE_READ_UNAVAILABLE"
                | "PREWRITE_PROTOCOL"
                | "INVALID_EMAIL"
        ) {
            return Err(MarketingConsentPersistenceError::InvalidInput);
        }
        let marker = format!("{RETRY_NO_WRITE_PREFIX}{reason_code}");
        let conn = tx.connection();
        lock_recipient(&mut *conn, &claim.intent.recipient_key).await?;
        // A release commit can succeed while its response is lost. Treat the exact
        // pending marker for this attempt as an idempotent confirmation; attempt_count
        // prevents an old claim from confirming a later retry's release.
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {TABLE} WHERE intent_id = $1 AND status = 'PENDING' AND last_error_code = $2 AND attempt_count = $3 AND completed_lease_token IS NULL AND completed_at IS NULL AND completion_status IS NULL AND provider_contact_id IS NULL AND email = $4 AND recipient_key = $5 AND desired = $6 AND source = $7 AND subject_type = $8 AND user_id IS NOT DISTINCT FROM $9 AND source_key = $10 AND consent_revision IS NOT DISTINCT FROM $11 AND changed_at = $12 AND not_after IS NOT DISTINCT FROM $13)"
        );
        let replay: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(claim.intent.intent_id.as_uuid())
            .bind(&marker)
            .bind(claim.attempt_count)
            .bind::<&str>(claim.intent.email.as_ref())
            .bind(&claim.intent.recipient_key)
            .bind(claim.intent.desired)
            .bind(bind_source(claim.intent.source))
            .bind(match claim.intent.subject {
                ConsentSubject::User(_) => "USER",
                ConsentSubject::EmailOnly => "EMAIL_ONLY",
            })
            .bind(match claim.intent.subject {
                ConsentSubject::User(id) => Some(*id.as_uuid()),
                ConsentSubject::EmailOnly => None,
            })
            .bind(&claim.intent.source_key)
            .bind(claim.intent.consent_revision)
            .bind(claim.intent.changed_at)
            .bind(claim.intent.not_after)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        if replay {
            return Ok(true);
        }
        let sql = format!(
            "UPDATE {TABLE} SET status = 'PENDING', lease_token = NULL, lease_expires_at = NULL, last_error_code = $3, updated = clock_timestamp() WHERE intent_id = $1 AND status = 'IN_PROGRESS' AND lease_token = $2 AND lease_expires_at > clock_timestamp()"
        );
        let updated = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(claim.intent.intent_id.as_uuid())
            .bind(claim.lease_token)
            .bind(marker)
            .execute(tx.connection())
            .await
            .map_err(db)?;
        Ok(updated.rows_affected() == 1)
    }
}

impl From<MarketingConsentIntentClaim> for ConsentWorkerClaim {
    fn from(claim: MarketingConsentIntentClaim) -> Self {
        let intent = claim.intent;
        Self {
            recipient_key: intent.recipient_key.clone(),
            consent_revision: intent.consent_revision,
            not_after: intent.not_after,
            changed_at: intent.changed_at,
            intent: intent.into(),
            lease_token: claim.lease_token.to_string(),
            lease_expires_at: claim.lease_expires_at,
            attempt_count: u32::try_from(claim.attempt_count)
                .expect("validated persisted attempt count"),
            prior_attempt_write_ambiguous: claim.prior_attempt_write_ambiguous,
        }
    }
}

fn worker_claim_from_port(
    claim: &ConsentWorkerClaim,
) -> Result<MarketingConsentIntentClaim, MarketingConsentPersistenceError> {
    let token = Uuid::parse_str(&claim.lease_token)
        .map_err(|_| MarketingConsentPersistenceError::InvalidInput)?;
    let intent = &claim.intent;
    Ok(MarketingConsentIntentClaim {
        intent: MarketingConsentIntent {
            intent_id: intent.intent_id,
            source_key: intent.source_key.clone(),
            subject: intent.subject,
            source: intent.source,
            email: intent.email.clone(),
            profile_snapshot: intent.profile_snapshot.as_deref().cloned(),
            recipient_key: claim.recipient_key.clone(),
            desired: intent.desired,
            consent_revision: claim.consent_revision,
            changed_at: claim.changed_at,
            status: ConsentIntentStatus::InProgress,
            not_after: claim.not_after,
        },
        lease_token: token,
        lease_expires_at: claim.lease_expires_at,
        attempt_count: i32::try_from(claim.attempt_count)
            .map_err(|_| MarketingConsentPersistenceError::InvalidInput)?,
        prior_attempt_write_ambiguous: claim.prior_attempt_write_ambiguous,
    })
}

#[async_trait::async_trait]
impl MarketingConsentIntentWorker<SqlxTransaction> for SqlxMarketingConsentIntentWorker {
    async fn claim_by_id(
        &self,
        tx: &mut SqlxTransaction,
        id: MarketingConsentSyncIntentId,
    ) -> Result<ConsentWorkerClaimOutcome, MarketingConsentIntentError> {
        Ok(SqlxMarketingConsentIntentWorker::claim_by_id(self, tx, id).await?)
    }

    async fn recheck(
        &self,
        tx: &mut SqlxTransaction,
        claim: &ConsentWorkerClaim,
    ) -> Result<ConsentWorkerRecheckOutcome, MarketingConsentIntentError> {
        let local = worker_claim_from_port(claim)?;
        if let Some(intent) = self.read_claim(tx, &local).await? {
            return Ok(ConsentWorkerRecheckOutcome::Ready(intent.into()));
        }
        let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE intent_id = $1");
        let row = sqlx::query_as::<_, IntentRow>(sqlx::AssertSqlSafe(sql))
            .bind(claim.intent.intent_id.as_uuid())
            .fetch_optional(tx.connection())
            .await
            .map_err(db)?;
        let Some(row) = row else {
            return Ok(ConsentWorkerRecheckOutcome::Missing);
        };
        let status = ConsentIntentStatus::parse(&row.status)?;
        row.into_intent()?;
        Ok(match status {
            ConsentIntentStatus::Applied => {
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Applied)
            }
            ConsentIntentStatus::Superseded => {
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Superseded)
            }
            ConsentIntentStatus::Blocked => {
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            }
            ConsentIntentStatus::Failed => {
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Failed)
            }
            ConsentIntentStatus::Pending | ConsentIntentStatus::InProgress => {
                ConsentWorkerRecheckOutcome::LeaseLost
            }
        })
    }

    async fn finalize_claim(
        &self,
        tx: &mut SqlxTransaction,
        claim: &ConsentWorkerClaim,
        result: ConsentWorkerFinalization<'_>,
        completed_at: OffsetDateTime,
    ) -> Result<bool, MarketingConsentIntentError> {
        let local = worker_claim_from_port(claim)?;
        Ok(self.finalize(tx, &local, result, completed_at).await?)
    }

    async fn release_for_retry(
        &self,
        tx: &mut SqlxTransaction,
        claim: &ConsentWorkerClaim,
        reason_code: &str,
    ) -> Result<bool, MarketingConsentIntentError> {
        let local = worker_claim_from_port(claim)?;
        Ok(
            SqlxMarketingConsentIntentWorker::release_for_retry(self, tx, &local, reason_code)
                .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_service_owned_recipient_key_for_validated_emails() {
        let mixed = Email::try_from("Ada@EXAMPLE.test").unwrap();
        let lower = Email::try_from("ada@example.TEST").unwrap();
        let key = marketing_consent_recipient_key(&mixed);
        assert_eq!(key, marketing_consent_recipient_key(&lower));
        assert_eq!(64, key.len());
        assert!(
            key.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
    }

    mod postgres {
        use super::{
            ConsentIntentSource, ConsentIntentStatus, ConsentSubject, MarketingConsentIntentClaim,
            MarketingConsentPersistenceError, SqlxMarketingConsentIntentRepository,
            SqlxMarketingConsentIntentWorker,
        };
        use crate::SqlxUserRepositoryFactory;
        use application::transaction::{Transaction, UnitOfWork};
        use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
        use serde_email::Email;
        use test_api::{
            IntegrationTestService, Postgres, aura_integration_test, get_postgres_client,
        };
        use time::{Duration, OffsetDateTime};
        use user_core::{
            marketing_consent_sync_intent_id::MarketingConsentSyncIntentId, user_id::UserId,
        };
        use user_service::ports::marketing_consent_intents::{
            ConsentWorkerClaimOutcome, ConsentWorkerFinalization, ConsentWorkerRecheckOutcome,
            ConsentWorkerTerminalStatus, MarketingConsentIntentWorker,
        };
        use user_service::ports::{
            ConsentIntentSource as PortSource, MarketingConsentIntentError,
            MarketingConsentIntents, MarketingConsentIntentsFactory, UserRepository,
            UserRepositoryFactory, UserStorageVersion,
        };
        use user_service::use_cases::commands::coordinate_marketing_consent::{
            GrantRaceRepairOutcome, MarketingConsentCoordinator,
        };

        const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
        fn email(value: &str) -> Email {
            Email::try_from(value).unwrap()
        }
        async fn seed(pool: &sqlx::PgPool, id: UserId, email: &Email) {
            sqlx::query(
                "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
            )
            .bind(id.as_uuid())
            .bind::<&str>(email.as_ref())
            .execute(pool)
            .await
            .unwrap();
        }

        async fn claim(
            worker: &SqlxMarketingConsentIntentWorker,
            tx: &mut SqlxTransaction,
            id: MarketingConsentSyncIntentId,
        ) -> MarketingConsentIntentClaim {
            let claimed = match MarketingConsentIntentWorker::claim_by_id(worker, tx, id)
                .await
                .unwrap()
            {
                ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                _ => panic!("requested intent was not claimed"),
            };
            let intent = SqlxMarketingConsentIntentRepository::new()
                .find_by_source_key(tx, &claimed.intent.source_key)
                .await
                .unwrap()
                .unwrap();
            MarketingConsentIntentClaim {
                intent,
                lease_token: uuid::Uuid::parse_str(&claimed.lease_token).unwrap(),
                lease_expires_at: claimed.lease_expires_at,
                attempt_count: i32::try_from(claimed.attempt_count).unwrap(),
                prior_attempt_write_ambiguous: claimed.prior_attempt_write_ambiguous,
            }
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn definite_no_write_release_preserves_exact_id_for_safe_retry() {
            let pool = get_postgres_client().await;
            let address = email(&format!("retry-{}@example.test", uuid::Uuid::now_v7()));
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repository = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let id = MarketingConsentCoordinator::new(&mut tx, &repository)
                .accepted_double_opt_in(
                    format!("retry-proof-{}", uuid::Uuid::now_v7()),
                    address,
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let claim = match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, id)
                .await
                .unwrap()
            {
                ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                _ => panic!("pending intent was not claimed"),
            };
            assert!(!claim.prior_attempt_write_ambiguous);
            assert!(
                MarketingConsentIntentWorker::release_for_retry(
                    &worker,
                    &mut tx,
                    &claim,
                    "THROTTLED",
                )
                .await
                .unwrap()
            );
            tx.commit().await.unwrap();

            // The caller retries this exact release after an unconfirmed commit response.
            // The persisted marker is confirmation, even though the live lease is gone.
            let mut replay_tx = uow.begin().await.unwrap();
            assert!(
                MarketingConsentIntentWorker::release_for_retry(
                    &worker,
                    &mut replay_tx,
                    &claim,
                    "THROTTLED"
                )
                .await
                .unwrap()
            );
            replay_tx.commit().await.unwrap();

            let persisted: (String, Option<String>, i32) = sqlx::query_as(
        "SELECT status, last_error_code, attempt_count FROM marketing_email_consent_sync_intents WHERE intent_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!("PENDING", persisted.0);
            assert_eq!(Some("RETRY_NO_WRITE:THROTTLED".to_owned()), persisted.1);
            assert_eq!(1, persisted.2);

            let mut tx = uow.begin().await.unwrap();
            let retried = match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, id)
                .await
                .unwrap()
            {
                ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                _ => panic!("released intent was not claimed again"),
            };
            assert_eq!(2, retried.attempt_count);
            assert!(!retried.prior_attempt_write_ambiguous);
            tx.commit().await.unwrap();

            let mut stale_release_tx = uow.begin().await.unwrap();
            assert!(
                !MarketingConsentIntentWorker::release_for_retry(
                    &worker,
                    &mut stale_release_tx,
                    &claim,
                    "THROTTLED"
                )
                .await
                .unwrap()
            );
            stale_release_tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn released_grant_superseded_by_withdrawal_remains_a_valid_terminal_intent() {
            let pool = get_postgres_client().await;
            let address = email(&format!(
                "retry-withdrawal-{}@example.test",
                uuid::Uuid::now_v7()
            ));
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repository = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();

            let mut tx = uow.begin().await.unwrap();
            let grant = MarketingConsentCoordinator::new(&mut tx, &repository)
                .accepted_double_opt_in(
                    format!("retry-withdrawal-proof-{}", uuid::Uuid::now_v7()),
                    address.clone(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let claim = match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, grant)
                .await
                .unwrap()
            {
                ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                _ => panic!("grant was not claimed"),
            };
            assert!(
                MarketingConsentIntentWorker::release_for_retry(
                    &worker,
                    &mut tx,
                    &claim,
                    "THROTTLED"
                )
                .await
                .unwrap()
            );
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let revoke = MarketingConsentCoordinator::new(&mut tx, &repository)
                .email_only_withdrawal(
                    address,
                    format!("retry-withdrawal-action-{}", uuid::Uuid::now_v7()),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, grant)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Superseded)
            ));
            tx.commit().await.unwrap();
            let old_row: (String, Option<String>) = sqlx::query_as(
        "SELECT status, last_error_code FROM marketing_email_consent_sync_intents WHERE intent_id = $1",
    )
    .bind(grant.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!(("SUPERSEDED".to_owned(), None), old_row);

            let mut tx = uow.begin().await.unwrap();
            let revoke_claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, revoke)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("newer withdrawal was not processable"),
                };
            assert!(!revoke_claim.intent.desired);
            tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn generic_user_transition_rejects_provider_race_repair_without_mutation() {
            let pool = get_postgres_client().await;
            let id = UserId::new();
            let address = email("reject-user-repair@example.test");
            seed(&pool, id, &address).await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                SqlxMarketingConsentIntentRepository::new()
                    .record_user_transition(
                        &mut tx,
                        id,
                        UserStorageVersion::INITIAL,
                        &address,
                        false,
                        ConsentIntentSource::ProviderRaceRepair,
                        "unauthorized-user-repair",
                        OffsetDateTime::now_utc(),
                    )
                    .await,
                Err(MarketingConsentPersistenceError::InvalidInput)
            ));
            tx.commit().await.unwrap();
            let state: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!(state, (false, 0, 1));
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
            )
            .bind::<&str>(address.as_ref())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count, 0);
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn generic_email_only_intent_rejects_provider_race_repair() {
            let pool = get_postgres_client().await;
            let address = email("reject-anonymous-repair@example.test");
            let uow = SqlxUnitOfWork::new(pool.clone());
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                SqlxMarketingConsentIntentRepository::new()
                    .record_email_only_intent(
                        &mut tx,
                        &address,
                        false,
                        ConsentIntentSource::ProviderRaceRepair,
                        "unauthorized-anonymous-repair",
                        OffsetDateTime::now_utc(),
                    )
                    .await,
                Err(MarketingConsentPersistenceError::InvalidInput)
            ));
            tx.commit().await.unwrap();
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
            )
            .bind::<&str>(address.as_ref())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count, 0);
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn verified_user_proofs_are_atomic_replay_safe_and_revision_fenced() {
            let pool = get_postgres_client().await;
            let id = UserId::new();
            let address = email("confirmed@example.test");
            seed(&pool, id, &address).await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let at = OffsetDateTime::now_utc();
            let mut tx = uow.begin().await.unwrap();
            let grant = repo
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "signup:proof",
                    at,
                )
                .await
                .unwrap();
            assert_eq!(ConsentSubject::User(id), grant.subject);
            assert_eq!(ConsentIntentSource::CognitoSignup, grant.source);
            assert_eq!(Some(1), grant.consent_revision);
            assert!(
                (grant.not_after.unwrap() - (at + Duration::days(7))).abs()
                    < Duration::milliseconds(1)
            );
            assert!(grant.intent_id.to_string().starts_with("mci_"));
            assert_eq!(64, grant.recipient_key.len());
            assert_eq!(
                grant.intent_id,
                MarketingConsentSyncIntentId::try_from(*grant.intent_id.as_uuid()).unwrap()
            );
            assert_eq!(
                grant.intent_id,
                repo.record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "signup:proof",
                    at
                )
                .await
                .unwrap()
                .intent_id
            );
            assert!(matches!(
                repo.record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    false,
                    ConsentIntentSource::UserWithdrawal,
                    "signup:proof",
                    at
                )
                .await,
                Err(MarketingConsentPersistenceError::SourceKeyConflict)
            ));
            assert!(matches!(
                repo.record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &email("wrong@example.test"),
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "wrong-proof",
                    at
                )
                .await,
                Err(MarketingConsentPersistenceError::ConcurrencyConflict)
            ));
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim = claim(&worker, &mut tx, grant.intent_id).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let newer = repo
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::try_from(2_i64).unwrap(),
                    &address,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "new-proof",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(Some(2), newer.consent_revision);
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(worker.read_claim(&mut tx, &claim).await.unwrap().is_none());
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let state: (bool, i64, i64) = sqlx::query_as("SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1")
        .bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!((true, 2, 3), state);
            assert_eq!(
                "SUPERSEDED",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(grant.intent_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn email_only_lease_receipt_and_provider_backsync_cancellation() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let address = email("Anonymous@EXAMPLE.test");
            let mut tx = uow.begin().await.unwrap();
            let grant = repo
                .record_email_only_intent(
                    &mut tx,
                    &address,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "double-opt-in",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(ConsentSubject::EmailOnly, grant.subject);
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let first = claim(&worker, &mut tx, grant.intent_id).await;
            assert_eq!(1, first.attempt_count);
            tx.commit().await.unwrap();
            sqlx::query("UPDATE marketing_email_consent_sync_intents SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE intent_id = $1")
        .bind(grant.intent_id.as_uuid()).execute(&pool).await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &first,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        first.lease_expires_at - Duration::seconds(1)
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            assert_eq!(
                "IN_PROGRESS",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(grant.intent_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            let mut tx = uow.begin().await.unwrap();
            let next = claim(&worker, &mut tx, grant.intent_id).await;
            assert_ne!(first.lease_token, next.lease_token);
            assert_eq!(2, next.attempt_count);
            assert!(worker.read_claim(&mut tx, &first).await.unwrap().is_none());
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &first,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert_eq!(
                1,
                repo.cancel_provider_backsync(&mut tx, &address)
                    .await
                    .unwrap()
            );
            assert_eq!(
                0,
                repo.cancel_provider_backsync(&mut tx, &address)
                    .await
                    .unwrap()
            );
            assert!(worker.read_claim(&mut tx, &next).await.unwrap().is_none());
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &next,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let revoke = repo
                .record_email_only_intent(
                    &mut tx,
                    &address,
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "anon-withdraw",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim = claim(&worker, &mut tx, revoke.intent_id).await;
            assert_eq!(revoke.intent_id, claim.intent.intent_id);
            tx.commit().await.unwrap();
            let completed_at = OffsetDateTime::now_utc();
            let applied = ConsentWorkerFinalization::Applied {
                provider_contact_id: Some("contact-1"),
            };
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize(&mut tx, &claim, applied, completed_at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize(&mut tx, &claim, applied, completed_at)
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        completed_at
                    )
                    .await
                    .unwrap()
            );
            let mut wrong_email = claim.clone();
            wrong_email.intent.email = email("anonymous@example.test");
            assert_ne!(wrong_email.intent.email, claim.intent.email);
            assert!(
                !worker
                    .finalize(&mut tx, &wrong_email, applied, completed_at)
                    .await
                    .unwrap()
            );
            assert_eq!(
                ConsentIntentStatus::Applied,
                repo.find_by_source_key(&mut tx, "anon-withdraw")
                    .await
                    .unwrap()
                    .unwrap()
                    .status
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn service_port_withdrawal_updates_registered_user_and_cancels_only_exact_email() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let factory = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let id = UserId::new();
            let address = email("Ada@example.test");
            let other_case = email("ada@example.test");
            assert_ne!(address, other_case);
            seed(&pool, id, &address).await;
            let mut tx = uow.begin().await.unwrap();
            let mut port = factory.in_transaction(&mut tx);
            assert!(
                port.find_user_by_email(&other_case)
                    .await
                    .unwrap()
                    .is_none()
            );
            let user = port.find_user_by_id(id).await.unwrap().unwrap();
            let grant = port
                .record_user_transition(
                    &user,
                    true,
                    PortSource::CognitoSignup,
                    "signup:ada",
                    None,
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(
                grant.intent_id,
                port.find_by_source_key("signup:ada")
                    .await
                    .unwrap()
                    .unwrap()
                    .intent_id
            );
            drop(port);
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let mut port = factory.in_transaction(&mut tx);
            port.cancel_provider_backsync(&other_case).await.unwrap();
            let user = port.find_user_by_email(&address).await.unwrap().unwrap();
            assert!(matches!(
                port.record_user_transition(
                    &user,
                    true,
                    PortSource::EmailOnlyWithdrawal,
                    "bad-source",
                    None,
                    OffsetDateTime::now_utc()
                )
                .await,
                Err(MarketingConsentIntentError::InvalidInput)
            ));
            assert!(matches!(
                port.record_user_transition(
                    &user,
                    true,
                    PortSource::AuraDoubleOptIn,
                    "signup:ada",
                    None,
                    OffsetDateTime::now_utc()
                )
                .await,
                Err(MarketingConsentIntentError::SourceKeyConflict)
            ));
            port.apply_provider_withdrawal(&user, OffsetDateTime::now_utc())
                .await
                .unwrap();
            port.cancel_provider_backsync(&address).await.unwrap();
            drop(port);
            tx.commit().await.unwrap();
            let state: (bool, i64, i64) = sqlx::query_as("SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1")
        .bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!((false, 2, 3), state);
            assert_eq!(
                1,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents"
                )
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, grant.intent_id)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            assert_eq!(
                ConsentIntentStatus::Blocked,
                factory
                    .find_by_source_key(&mut tx, "signup:ada")
                    .await
                    .unwrap()
                    .unwrap()
                    .status
            );
            drop(tx);
            let mut tx = uow.begin().await.unwrap();
            let mut port = factory.in_transaction(&mut tx);
            assert!(matches!(
                port.apply_provider_withdrawal(&user, OffsetDateTime::now_utc())
                    .await,
                Err(MarketingConsentIntentError::ConcurrencyConflict)
            ));
            let current = port.find_user_by_email(&address).await.unwrap().unwrap();
            port.apply_provider_withdrawal(&current, OffsetDateTime::now_utc())
                .await
                .unwrap();
            drop(port);
            tx.commit().await.unwrap();
            let unchanged: (i64, i64) = sqlx::query_as(
                "SELECT marketing_email_consent_revision, version FROM users WHERE user_id = $1",
            )
            .bind(id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!((2, 3), unchanged);
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn case_folded_lock_does_not_merge_distinct_exact_provider_addresses() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let first_email = email("Case@example.test");
            let second_email = email("case@example.test");
            let mut tx = uow.begin().await.unwrap();
            let first = repo
                .record_email_only_intent(
                    &mut tx,
                    &first_email,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "first-case",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            let second = repo
                .record_email_only_intent(
                    &mut tx,
                    &second_email,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "second-case",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(first.recipient_key, second.recipient_key);
            assert_ne!(first.email, second.email);
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let first_claim = claim(&worker, &mut tx, first.intent_id).await;
            assert_eq!(first.intent_id, first_claim.intent.intent_id);
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert_eq!(
                1,
                repo.cancel_provider_backsync(&mut tx, &second_email)
                    .await
                    .unwrap()
            );
            assert!(
                worker
                    .read_claim(&mut tx, &first_claim)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(
                ConsentIntentStatus::Blocked,
                repo.find_by_source_key(&mut tx, "second-case")
                    .await
                    .unwrap()
                    .unwrap()
                    .status
            );
            tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn local_verified_grant_and_intent_roll_back_with_callers_transaction() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let factory = SqlxMarketingConsentIntentRepository::new();
            let id = UserId::new();
            let address = email("rollback-local-grant@example.test");
            seed(&pool, id, &address).await;
            let initial: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!((false, 0, 1), initial);

            let mut tx = uow.begin().await.unwrap();
            let intent_id = MarketingConsentCoordinator::new(&mut tx, &factory)
                .accepted_double_opt_in(
                    "rollback-local-proof".to_string(),
                    address,
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            let inside: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(tx.connection())
    .await
    .unwrap();
            assert_eq!((true, 1, 2), inside);
            assert_eq!(
                1_i64,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(intent_id.as_uuid())
                .fetch_one(tx.connection())
                .await
                .unwrap()
            );
            let source_key: String = sqlx::query_scalar(
                "SELECT source_key FROM marketing_email_consent_sync_intents WHERE intent_id = $1",
            )
            .bind(intent_id.as_uuid())
            .fetch_one(tx.connection())
            .await
            .unwrap();
            drop(tx);

            let after: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!(initial, after);
            assert_eq!(
        0_i64,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE intent_id = $1 OR source_key = $2"
        )
        .bind(intent_id.as_uuid())
        .bind(source_key)
        .fetch_one(&pool)
        .await
        .unwrap()
    );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn provider_withdrawal_rolls_back_both_user_change_and_cancellation() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let factory = SqlxMarketingConsentIntentRepository::new();
            let address = email("rollback-provider@example.test");
            let id = UserId::new();
            seed(&pool, id, &address).await;
            let mut tx = uow.begin().await.unwrap();
            let mut port = factory.in_transaction(&mut tx);
            let user = port.find_user_by_id(id).await.unwrap().unwrap();
            port.record_user_transition(
                &user,
                true,
                PortSource::CognitoSignup,
                "grant-for-rollback",
                None,
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
            drop(port);
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let mut port = factory.in_transaction(&mut tx);
            let user = port.find_user_by_email(&address).await.unwrap().unwrap();
            port.apply_provider_withdrawal(&user, OffsetDateTime::now_utc())
                .await
                .unwrap();
            port.cancel_provider_backsync(&address).await.unwrap();
            drop(port);
            drop(tx);
            assert!(
                sqlx::query_scalar::<_, bool>(
                    "SELECT marketing_email_consent FROM users WHERE user_id = $1"
                )
                .bind(id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            let mut tx = uow.begin().await.unwrap();
            assert_eq!(
                ConsentIntentStatus::Pending,
                factory
                    .find_by_source_key(&mut tx, "grant-for-rollback")
                    .await
                    .unwrap()
                    .unwrap()
                    .status
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn deletion_cancels_user_grant_and_preserves_user_revoke() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let id = UserId::new();
            let address = email("deleted@example.test");
            seed(&pool, id, &address).await;
            let mut tx = uow.begin().await.unwrap();
            let grant = repo
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "signup:deleted",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let grant_claim = claim(&worker, &mut tx, grant.intent_id).await;
            assert_eq!(grant.intent_id, grant_claim.intent.intent_id);
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let revoke = repo
                .record_user_deletion(
                    &mut tx,
                    id,
                    UserStorageVersion::try_from(2_i64).unwrap(),
                    &address,
                    "delete:account",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(ConsentSubject::User(id), revoke.subject);
            assert_eq!(ConsentIntentSource::UserDeletion, revoke.source);
            assert_eq!(address, revoke.email);
            assert!(!revoke.desired);
            assert_eq!(Some(2), revoke.consent_revision);
            tx.commit().await.unwrap();
            assert!(
                !sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM users WHERE user_id = $1)"
                )
                .bind(id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .read_claim(&mut tx, &grant_claim)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &grant_claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            assert_eq!(
                revoke.intent_id,
                repo.record_user_deletion(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    "delete:account",
                    OffsetDateTime::now_utc()
                )
                .await
                .unwrap()
                .intent_id
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let revoke_claim = claim(&worker, &mut tx, revoke.intent_id).await;
            assert_eq!(revoke.intent_id, revoke_claim.intent.intent_id);
            assert!(
                worker
                    .read_claim(&mut tx, &revoke_claim)
                    .await
                    .unwrap()
                    .is_some()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize(
                        &mut tx,
                        &revoke_claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            assert_eq!(
                "APPLIED",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(revoke.intent_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            assert_eq!(1, sqlx::query_scalar::<_, i64>("SELECT count(*) FROM marketing_email_consent_sync_intents WHERE user_id = $1 AND NOT desired")
        .bind(id.as_uuid()).fetch_one(&pool).await.unwrap());
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn anonymous_grant_cannot_override_registered_email() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let address = email("later@example.test");
            let mut tx = uow.begin().await.unwrap();
            let grant = repo
                .record_email_only_intent(
                    &mut tx,
                    &address,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "anonymous",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            seed(&pool, UserId::new(), &address).await;
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, grant.intent_id)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            assert!(matches!(
                repo.record_email_only_intent(
                    &mut tx,
                    &address,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "another",
                    OffsetDateTime::now_utc()
                )
                .await,
                Err(MarketingConsentPersistenceError::RegisteredEmail)
            ));
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn concurrent_same_proof_commits_one_intent_and_one_consent_revision() {
            let pool = get_postgres_client().await;
            let id = UserId::new();
            let address = email("concurrent-proof@example.test");
            seed(&pool, id, &address).await;

            async fn accept(
                pool: sqlx::PgPool,
                id: UserId,
                address: Email,
            ) -> MarketingConsentSyncIntentId {
                let uow = SqlxUnitOfWork::new(pool);
                let mut tx = uow.begin().await.unwrap();
                let intent = SqlxMarketingConsentIntentRepository::new()
                    .record_user_transition(
                        &mut tx,
                        id,
                        UserStorageVersion::INITIAL,
                        &address,
                        true,
                        ConsentIntentSource::CognitoSignup,
                        "signup:concurrent-proof",
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
                intent.intent_id
            }

            let (first, second) = tokio::join!(
                accept(pool.clone(), id, address.clone()),
                accept(pool.clone(), id, address),
            );
            assert_eq!(first, second);
            let state: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!((true, 1, 2), state);
            assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE source_key = $1"
        )
        .bind("signup:concurrent-proof")
        .fetch_one(&pool)
        .await
        .unwrap()
    );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn ordinary_profile_update_does_not_advance_revision_between_distinct_proofs() {
            use user_core::user::UserProfile;

            let pool = get_postgres_client().await;
            let id = UserId::new();
            let address = email("profile-proof@example.test");
            seed(&pool, id, &address).await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let intents = SqlxMarketingConsentIntentRepository::new();
            let users = SqlxUserRepositoryFactory::new();
            let mut tx = uow.begin().await.unwrap();
            let first = intents
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "signup:profile-proof",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let loaded = users
                .in_transaction(&mut tx)
                .find_by_id(id)
                .await
                .unwrap()
                .unwrap();
            let mut changed = loaded.value;
            changed
                .replace_profile(UserProfile {
                    first_name: Some("Updated".into()),
                    last_name: None,
                })
                .unwrap();
            let updated = users
                .in_transaction(&mut tx)
                .update(&changed, loaded.version)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let after_profile: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!((true, 1, 3), after_profile);
            assert_eq!(Some(1), first.consent_revision);
            assert_eq!(
                "Updated",
                sqlx::query_scalar::<_, String>("SELECT first_name FROM users WHERE user_id = $1")
                    .bind(id.as_uuid())
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            );

            let mut tx = uow.begin().await.unwrap();
            let second = intents
                .record_user_transition(
                    &mut tx,
                    id,
                    updated.version,
                    &address,
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "doi:profile-proof",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            assert_ne!(first.intent_id, second.intent_id);
            assert_eq!(Some(2), second.consent_revision);
            let after_second: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!((true, 2, 4), after_second);
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn old_signup_replay_after_revoke_remains_revoked() {
            let pool = get_postgres_client().await;
            let id = UserId::new();
            let address = email("withdrawn-signup@example.test");
            seed(&pool, id, &address).await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let grant = repo
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "signup:withdrawn",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let revoke = repo
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::try_from(2_i64).unwrap(),
                    &address,
                    false,
                    ConsentIntentSource::UserWithdrawal,
                    "withdraw:signup",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let replay = repo
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "signup:withdrawn",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(grant.intent_id, replay.intent_id);
            assert_eq!(ConsentIntentStatus::Superseded, replay.status);
            tx.commit().await.unwrap();
            let current: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!((false, 2, 3), current);
            assert_eq!(
                2,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE user_id = $1"
                )
                .bind(id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            let mut tx = uow.begin().await.unwrap();
            let next = claim(&worker, &mut tx, revoke.intent_id).await;
            assert_eq!(revoke.intent_id, next.intent.intent_id);
            assert!(!next.intent.desired);
        }

        #[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
        struct PersistedCompletion {
            status: String,
            lease_token: Option<uuid::Uuid>,
            lease_expires_at: Option<OffsetDateTime>,
            completed_lease_token: Option<uuid::Uuid>,
            completed_at: Option<OffsetDateTime>,
            provider_contact_id: Option<String>,
            last_error_code: Option<String>,
            attempt_count: i32,
            updated: OffsetDateTime,
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn finalize_exact_retry_keeps_receipt_and_conflicting_completion_cannot_succeed() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let address = email("receipt@example.test");
            let mut tx = uow.begin().await.unwrap();
            let intent = repo
                .record_email_only_intent(
                    &mut tx,
                    &address,
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "withdraw:receipt",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim = claim(&worker, &mut tx, intent.intent_id).await;
            tx.commit().await.unwrap();

            let completed_at = OffsetDateTime::now_utc();
            let applied = ConsentWorkerFinalization::Applied {
                provider_contact_id: Some("contact-receipt"),
            };
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize(&mut tx, &claim, applied, completed_at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let persisted = async |pool: &sqlx::PgPool| -> PersistedCompletion {
                sqlx::query_as::<_, PersistedCompletion>("SELECT status, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_contact_id, last_error_code, attempt_count, updated FROM marketing_email_consent_sync_intents WHERE intent_id = $1")
            .bind(claim.intent.intent_id.as_uuid()).fetch_one(pool).await.unwrap()
            };
            let receipt = persisted(&pool).await;
            assert_eq!("APPLIED", receipt.status);
            assert_eq!(Some(claim.lease_token), receipt.completed_lease_token);
            assert!(receipt.lease_token.is_none());
            assert!(receipt.lease_expires_at.is_none());
            assert_eq!(
                Some("contact-receipt".to_owned()),
                receipt.provider_contact_id
            );
            assert!(receipt.last_error_code.is_none());
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize(&mut tx, &claim, applied, completed_at)
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &claim,
                        applied,
                        completed_at + Duration::seconds(1)
                    )
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: Some("different-contact")
                        },
                        completed_at
                    )
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Failed {
                            error_code: "provider_failed"
                        },
                        completed_at
                    )
                    .await
                    .unwrap()
            );
            let mut wrong_token = claim.clone();
            wrong_token.lease_token = uuid::Uuid::now_v7();
            assert!(
                !worker
                    .finalize(&mut tx, &wrong_token, applied, completed_at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            assert_eq!(receipt, persisted(&pool).await);
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn typed_worker_claim_distinguishes_missing_deferred_terminal_and_exact_receipt() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(
                    &worker,
                    &mut tx,
                    MarketingConsentSyncIntentId::new()
                )
                .await
                .unwrap(),
                ConsentWorkerClaimOutcome::Missing
            ));
            let intent = repo
                .record_email_only_intent(
                    &mut tx,
                    &email("typed-worker@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:typed",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("pending intent was not claimed"),
                };
            assert_eq!(claim.intent.intent_id, intent.intent_id);
            assert_eq!(claim.attempt_count, 1);
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id).await.unwrap(),
                ConsentWorkerClaimOutcome::Deferred { lease_expires_at } if lease_expires_at == claim.lease_expires_at
            ));
            assert!(matches!(
                worker.recheck(&mut tx, &claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Ready(ref decision) if decision.intent_id == intent.intent_id && !decision.desired
            ));
            tx.commit().await.unwrap();
            let completed_at = OffsetDateTime::now_utc();
            let result = ConsentWorkerFinalization::Applied {
                provider_contact_id: Some("contact-typed"),
            };
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize_claim(&mut tx, &claim, result, completed_at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            for _ in 0..2 {
                assert!(matches!(
                    MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                        .await
                        .unwrap(),
                    ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Applied)
                ));
            }
            assert!(matches!(
                worker.recheck(&mut tx, &claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Applied)
            ));
            assert!(
                worker
                    .finalize_claim(&mut tx, &claim, result, completed_at)
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize_claim(&mut tx, &claim, result, completed_at + Duration::seconds(1))
                    .await
                    .unwrap()
            );
            let mut stale = claim.clone();
            stale.lease_token = uuid::Uuid::now_v7().to_string();
            assert!(
                !worker
                    .finalize_claim(&mut tx, &stale, result, completed_at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn typed_worker_recheck_blocks_reassigned_deletion_revoke_and_fences_finalization() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let old_user = UserId::new();
            let address = email("typed-reassigned@example.test");
            seed(&pool, old_user, &address).await;
            let mut tx = uow.begin().await.unwrap();
            let revoke = repo
                .record_user_deletion(
                    &mut tx,
                    old_user,
                    UserStorageVersion::INITIAL,
                    &address,
                    "worker:delete",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, revoke.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("revoke was not claimed"),
                };
            tx.commit().await.unwrap();
            seed(&pool, UserId::new(), &address).await;
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                worker.recheck(&mut tx, &claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, revoke.intent_id)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn typed_worker_expired_grant_blocks_and_expired_lease_reclaims_without_old_custody()
        {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let grant = repo
                .record_email_only_intent(
                    &mut tx,
                    &email("expired-grant@example.test"),
                    true,
                    ConsentIntentSource::AuraDoubleOptIn,
                    "worker:expired",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            let revoke = repo
                .record_email_only_intent(
                    &mut tx,
                    &email("reclaim-revoke@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:reclaim",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            sqlx::query("UPDATE marketing_email_consent_sync_intents SET changed_at = changed_at - interval '8 days', not_after = not_after - interval '8 days' WHERE intent_id = $1")
        .bind(grant.intent_id.as_uuid()).execute(&pool).await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, grant.intent_id)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            let first =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, revoke.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("revoke was not claimed"),
                };
            tx.commit().await.unwrap();
            sqlx::query("UPDATE marketing_email_consent_sync_intents SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE intent_id = $1")
        .bind(revoke.intent_id.as_uuid()).execute(&pool).await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let second =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, revoke.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("expired revoke lease was not reclaimed"),
                };
            assert_eq!(second.attempt_count, 2);
            assert_ne!(first.lease_token, second.lease_token);
            assert!(matches!(
                worker.recheck(&mut tx, &first).await.unwrap(),
                ConsentWorkerRecheckOutcome::LeaseLost
            ));
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &first,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            assert!(matches!(
                worker.recheck(&mut tx, &second).await.unwrap(),
                ConsentWorkerRecheckOutcome::Ready(_)
            ));
            tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn blocked_finalization_has_exact_receipt_and_rejects_conflicting_tuple() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let intent = SqlxMarketingConsentIntentRepository::new()
                .record_email_only_intent(
                    &mut tx,
                    &email("blocked-receipt@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:blocked-receipt",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("pending revoke was not claimed"),
                };
            tx.commit().await.unwrap();
            let completed_at = OffsetDateTime::now_utc();
            let blocked = ConsentWorkerFinalization::Blocked {
                error_code: "provider_opted_out",
            };
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize_claim(&mut tx, &claim, blocked, completed_at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();

            let receipt = sqlx::query_as::<_, PersistedCompletion>(
        "SELECT status, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_contact_id, last_error_code, attempt_count, updated FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(intent.intent_id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(receipt.status, "BLOCKED");
            assert_eq!(
                receipt.completed_lease_token.map(|token| token.to_string()),
                Some(claim.lease_token.clone())
            );
            assert!(
                (receipt.completed_at.unwrap() - completed_at).abs() < Duration::microseconds(1)
            );
            assert_eq!(
                receipt.last_error_code.as_deref(),
                Some("provider_opted_out")
            );
            assert!(receipt.provider_contact_id.is_none());
            assert!(receipt.lease_token.is_none() && receipt.lease_expires_at.is_none());
            assert_eq!(receipt.attempt_count, 1);
            assert_eq!(sqlx::query_scalar::<_, String>(
        "SELECT completion_status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(intent.intent_id.as_uuid()).fetch_one(&pool).await.unwrap(), "BLOCKED");

            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize_claim(&mut tx, &claim, blocked, completed_at)
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        blocked,
                        completed_at + Duration::seconds(1)
                    )
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Blocked {
                            error_code: "different_code"
                        },
                        completed_at
                    )
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Failed {
                            error_code: "provider_opted_out"
                        },
                        completed_at
                    )
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        completed_at
                    )
                    .await
                    .unwrap()
            );
            let mut other_token = claim.clone();
            other_token.lease_token = uuid::Uuid::now_v7().to_string();
            assert!(
                !worker
                    .finalize_claim(&mut tx, &other_token, blocked, completed_at)
                    .await
                    .unwrap()
            );
            let mut other_target = claim.clone();
            other_target.intent.source_key = "worker:different-proof".to_owned();
            assert!(
                !worker
                    .finalize_claim(&mut tx, &other_target, blocked, completed_at)
                    .await
                    .unwrap()
            );
            for _ in 0..2 {
                assert!(matches!(
                    MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                        .await
                        .unwrap(),
                    ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
                ));
            }
            assert!(matches!(
                worker.recheck(&mut tx, &claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            tx.commit().await.unwrap();
            assert_eq!(receipt, sqlx::query_as::<_, PersistedCompletion>(
        "SELECT status, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_contact_id, last_error_code, attempt_count, updated FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(intent.intent_id.as_uuid()).fetch_one(&pool).await.unwrap());
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn typed_claim_selects_only_requested_id_and_missing_does_not_claim_other_rows() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let a = repo
                .record_email_only_intent(
                    &mut tx,
                    &email("choose-a@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:choose-a",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            let b = repo
                .record_email_only_intent(
                    &mut tx,
                    &email("choose-b@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:choose-b",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(
                    &worker,
                    &mut tx,
                    MarketingConsentSyncIntentId::new()
                )
                .await
                .unwrap(),
                ConsentWorkerClaimOutcome::Missing
            ));
            let b_claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, b.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("B was not claimed"),
                };
            assert_eq!(b_claim.intent.intent_id, b.intent_id);
            tx.commit().await.unwrap();
            let rows: Vec<(uuid::Uuid, String, i32)> = sqlx::query_as(
        "SELECT intent_id, status, attempt_count FROM marketing_email_consent_sync_intents ORDER BY intent_sequence"
    ).fetch_all(&pool).await.unwrap();
            assert_eq!(
                rows,
                vec![
                    (*a.intent_id.as_uuid(), "PENDING".into(), 0),
                    (*b.intent_id.as_uuid(), "IN_PROGRESS".into(), 1)
                ]
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn simultaneous_claims_for_same_id_have_one_committed_lease_and_one_deferred() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let mut tx = uow.begin().await.unwrap();
            let intent = SqlxMarketingConsentIntentRepository::new()
                .record_email_only_intent(
                    &mut tx,
                    &email("same-id@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:same-id",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            async fn compete(
                pool: sqlx::PgPool,
                id: MarketingConsentSyncIntentId,
                hold: bool,
            ) -> ConsentWorkerClaimOutcome {
                let uow = SqlxUnitOfWork::new(pool);
                let mut tx = uow.begin().await.unwrap();
                let outcome = MarketingConsentIntentWorker::claim_by_id(
                    &SqlxMarketingConsentIntentWorker::new(),
                    &mut tx,
                    id,
                )
                .await
                .unwrap();
                if hold {
                    tokio::task::yield_now().await;
                }
                tx.commit().await.unwrap();
                outcome
            }
            let (first, second) = tokio::join!(
                compete(pool.clone(), intent.intent_id, true),
                compete(pool.clone(), intent.intent_id, false)
            );
            let (claim, deferred) = match (first, second) {
                (
                    ConsentWorkerClaimOutcome::Claimed(claim),
                    ConsentWorkerClaimOutcome::Deferred { lease_expires_at },
                )
                | (
                    ConsentWorkerClaimOutcome::Deferred { lease_expires_at },
                    ConsentWorkerClaimOutcome::Claimed(claim),
                ) => (claim, lease_expires_at),
                _ => panic!("same-ID contenders must have one winner and one deferral"),
            };
            assert_eq!(claim.intent.intent_id, intent.intent_id);
            assert_eq!(deferred, claim.lease_expires_at);
            let persisted: (i32, Option<uuid::Uuid>, OffsetDateTime) = sqlx::query_as(
        "SELECT attempt_count, lease_token, lease_expires_at FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(intent.intent_id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(persisted.0, 1);
            assert_eq!(
                persisted.1.map(|token| token.to_string()),
                Some(claim.lease_token)
            );
            assert_eq!(persisted.2, claim.lease_expires_at);
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn stale_user_revision_recheck_supersedes_claim_and_rejects_finalize() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let id = UserId::new();
            let address = email("stale-worker-user@example.test");
            seed(&pool, id, &address).await;
            let mut tx = uow.begin().await.unwrap();
            let intent = SqlxMarketingConsentIntentRepository::new()
                .record_user_transition(
                    &mut tx,
                    id,
                    UserStorageVersion::INITIAL,
                    &address,
                    true,
                    ConsentIntentSource::CognitoSignup,
                    "worker:stale-user",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("grant was not claimed"),
                };
            assert_eq!(claim.consent_revision, Some(1));
            tx.commit().await.unwrap();
            // Simulate an independently advanced authoritative User decision without a newer intent.
            sqlx::query("UPDATE users SET marketing_email_consent = false, marketing_email_consent_revision = marketing_email_consent_revision + 1 WHERE user_id = $1")
        .bind(id.as_uuid()).execute(&pool).await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                worker.recheck(&mut tx, &claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Superseded)
            ));
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            for _ in 0..2 {
                assert!(matches!(
                    MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                        .await
                        .unwrap(),
                    ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Superseded)
                ));
            }
            assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT attempt_count FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
        )
        .bind(intent.intent_id.as_uuid())
        .fetch_one(tx.connection())
        .await
        .unwrap(),
        1
    );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn failed_terminal_replays_without_reclaiming_or_rewriting_receipt() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let worker = SqlxMarketingConsentIntentWorker::new();
            let mut tx = uow.begin().await.unwrap();
            let intent = SqlxMarketingConsentIntentRepository::new()
                .record_email_only_intent(
                    &mut tx,
                    &email("failed-terminal@example.test"),
                    false,
                    ConsentIntentSource::EmailOnlyWithdrawal,
                    "worker:failed",
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("pending intent was not claimed"),
                };
            tx.commit().await.unwrap();
            let at = OffsetDateTime::now_utc();
            let failure = ConsentWorkerFinalization::Failed {
                error_code: "permanent_provider_failure",
            };
            let mut tx = uow.begin().await.unwrap();
            assert!(
                worker
                    .finalize_claim(&mut tx, &claim, failure, at)
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            let receipt = sqlx::query_as::<_, PersistedCompletion>(
        "SELECT status, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_contact_id, last_error_code, attempt_count, updated FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(intent.intent_id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(receipt.status, "FAILED");
            assert_eq!(
                receipt.last_error_code.as_deref(),
                Some("permanent_provider_failure")
            );
            let mut tx = uow.begin().await.unwrap();
            for _ in 0..2 {
                assert!(matches!(
                    MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, intent.intent_id)
                        .await
                        .unwrap(),
                    ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Failed)
                ));
            }
            assert!(matches!(
                worker.recheck(&mut tx, &claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Failed)
            ));
            assert!(
                worker
                    .finalize_claim(&mut tx, &claim, failure, at)
                    .await
                    .unwrap()
            );
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &claim,
                        ConsentWorkerFinalization::Blocked {
                            error_code: "permanent_provider_failure"
                        },
                        at
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            assert_eq!(receipt, sqlx::query_as::<_, PersistedCompletion>(
        "SELECT status, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_contact_id, last_error_code, attempt_count, updated FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(intent.intent_id.as_uuid()).fetch_one(&pool).await.unwrap());
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn provider_withdrawal_racing_user_grant_schedules_one_revoke_and_replays_it() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let id = UserId::new();
            let address = email("race-provider-user@example.test");
            seed(&pool, id, &address).await;
            let now = OffsetDateTime::now_utc();
            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-provider-user-proof".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let in_flight = claim(&worker, &mut tx, original).await;
            assert_eq!(ConsentSubject::User(id), in_flight.intent.subject);
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .provider_withdrawal(address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, original)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            assert!(
                !worker
                    .finalize(
                        &mut tx,
                        &in_flight,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None,
                        },
                        OffsetDateTime::now_utc()
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();

            // A provider grant sent under the old lease cannot be undone by the failed finalize.
            let mut tx = uow.begin().await.unwrap();
            let repair = match MarketingConsentCoordinator::new(&mut tx, &repo)
                .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                .await
                .unwrap()
            {
                GrantRaceRepairOutcome::RepairScheduled(id) => id,
                _ => panic!("raced provider withdrawal must schedule a revoke"),
            };
            assert_ne!(original, repair);
            #[derive(sqlx::FromRow)]
            struct RepairRow {
                source: String,
                subject_type: String,
                user_id: Option<uuid::Uuid>,
                email: String,
                desired: bool,
                consent_revision: Option<i64>,
                status: String,
                not_after: Option<OffsetDateTime>,
            }
            let row: RepairRow = sqlx::query_as(
        "SELECT source, subject_type, user_id, email, desired, consent_revision, status, not_after FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(repair.as_uuid()).fetch_one(tx.connection()).await.unwrap();
            assert_eq!(row.source, "PROVIDER_RACE_REPAIR");
            assert_eq!(row.subject_type, "USER");
            assert_eq!(row.user_id, Some(*id.as_uuid()));
            assert_eq!(row.email, <Email as AsRef<str>>::as_ref(&address));
            assert!(!row.desired);
            assert_eq!(row.consent_revision, Some(2));
            assert_eq!(row.status, "PENDING");
            assert_eq!(row.not_after, None);
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::ExistingRepair(id) if id == repair
            ));
            tx.commit().await.unwrap();
            assert_eq!(
                2,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1"
                )
                .bind::<&str>(address.as_ref())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            let state: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1"
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(state, (false, 2, 3));
            let mut tx = uow.begin().await.unwrap();
            let repair_claim = claim(&worker, &mut tx, repair).await;
            assert!(!repair_claim.intent.desired);
            assert_eq!(
                repair_claim.intent.source,
                ConsentIntentSource::ProviderRaceRepair
            );
            let port_intent = repo
                .in_transaction(&mut tx)
                .find_by_source_key(&repair_claim.intent.source_key)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(port_intent.intent_id, repair);
            assert_eq!(port_intent.source, PortSource::ProviderRaceRepair);
            tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn concurrent_repairs_of_same_raced_grant_share_one_revoke() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let id = UserId::new();
            let address = email("concurrent-provider-repair@example.test");
            seed(&pool, id, &address).await;
            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in(
                    "concurrent-provider-repair-proof".into(),
                    address.clone(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let claim = match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, original)
                .await
                .unwrap()
            {
                ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                _ => panic!("grant must be leased"),
            };
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .provider_withdrawal(address.clone(), OffsetDateTime::now_utc())
                .await
                .unwrap();
            tx.commit().await.unwrap();

            async fn repair(
                pool: sqlx::PgPool,
                original: MarketingConsentSyncIntentId,
                claim: Option<user_service::ports::marketing_consent_intents::ConsentWorkerClaim>,
                barrier: std::sync::Arc<tokio::sync::Barrier>,
            ) -> GrantRaceRepairOutcome {
                let uow = SqlxUnitOfWork::new(pool);
                let mut tx = uow.begin().await.unwrap();
                barrier.wait().await;
                if let Some(claim) = claim {
                    assert!(matches!(
                        MarketingConsentIntentWorker::recheck(
                            &SqlxMarketingConsentIntentWorker::new(),
                            &mut tx,
                            &claim,
                        )
                        .await
                        .unwrap(),
                        ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
                    ));
                }
                let result = MarketingConsentCoordinator::new(
                    &mut tx,
                    &SqlxMarketingConsentIntentRepository::new(),
                )
                .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                .await
                .unwrap();
                tx.commit().await.unwrap();
                result
            }

            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let (first, second) = tokio::join!(
                repair(pool.clone(), original, Some(claim), barrier.clone()),
                repair(pool.clone(), original, None, barrier),
            );
            let (scheduled, existing) = match (first, second) {
                (
                    GrantRaceRepairOutcome::RepairScheduled(id),
                    GrantRaceRepairOutcome::ExistingRepair(other),
                )
                | (
                    GrantRaceRepairOutcome::ExistingRepair(other),
                    GrantRaceRepairOutcome::RepairScheduled(id),
                ) => (id, other),
                _ => panic!("concurrent repair must schedule once and reuse the same revoke"),
            };
            assert_eq!(scheduled, existing);
            let persisted: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT intent_id FROM marketing_email_consent_sync_intents WHERE source = 'PROVIDER_RACE_REPAIR' AND email = $1",
    )
    .bind::<&str>(address.as_ref())
    .fetch_all(&pool)
    .await
    .unwrap();
            assert_eq!(persisted, vec![*scheduled.as_uuid()]);
            let state: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
            assert_eq!(state, (false, 2, 3));
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn fresh_verified_grant_after_provider_withdrawal_needs_no_race_repair() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let user_id = UserId::new();
            let address = email("race-provider-fresh-grant@example.test");
            seed(&pool, user_id, &address).await;
            let now = OffsetDateTime::now_utc();

            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-provider-fresh-first".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert_eq!(
                ConsentSubject::User(user_id),
                claim(&worker, &mut tx, original).await.intent.subject
            );
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .provider_withdrawal(address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let fresh = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in(
                    "race-provider-fresh-second".into(),
                    address.clone(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::NoRepairNeeded
            ));
            tx.commit().await.unwrap();
            assert_eq!(0, sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE source = 'PROVIDER_RACE_REPAIR' AND email = $1"
    ).bind::<&str>(address.as_ref()).fetch_one(&pool).await.unwrap());
            assert_eq!(
                "PENDING",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(fresh.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn newer_verified_grant_supersedes_leased_user_race_repair_on_recheck() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let user_id = UserId::new();
            let address = email("race-repair-stale-revoke@example.test");
            seed(&pool, user_id, &address).await;
            let now = OffsetDateTime::now_utc();

            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-repair-stale-first".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            claim(&worker, &mut tx, original).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .provider_withdrawal(address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let repair = match MarketingConsentCoordinator::new(&mut tx, &repo)
                .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                .await
                .unwrap()
            {
                GrantRaceRepairOutcome::RepairScheduled(id) => id,
                _ => panic!("expected a compensating USER revoke"),
            };
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let repair_claim =
                match MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, repair)
                    .await
                    .unwrap()
                {
                    ConsentWorkerClaimOutcome::Claimed(claim) => claim,
                    _ => panic!("repair revoke was not claimed"),
                };
            assert!(!repair_claim.intent.desired);
            tx.commit().await.unwrap();

            let mut tx = uow.begin().await.unwrap();
            let fresh = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in(
                    "race-repair-stale-second".into(),
                    address.clone(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                worker.recheck(&mut tx, &repair_claim).await.unwrap(),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Superseded)
            ));
            assert!(
                !worker
                    .finalize_claim(
                        &mut tx,
                        &repair_claim,
                        ConsentWorkerFinalization::Applied {
                            provider_contact_id: None
                        },
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
            assert_eq!(
                "SUPERSEDED",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(repair.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            assert_eq!(
                "PENDING",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(fresh.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn raced_user_grant_reuses_local_revoke_but_not_after_a_fresh_grant() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let id = UserId::new();
            let address = email("race-local-revoke@example.test");
            seed(&pool, id, &address).await;
            let now = OffsetDateTime::now_utc();
            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-local-first".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            claim(&worker, &mut tx, original).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let revoke = MarketingConsentCoordinator::new(&mut tx, &repo)
                .user_withdrawal(id, address.clone(), "race-local-withdraw".into(), now)
                .await
                .unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, now)
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::ExistingRepair(found) if found == revoke
            ));
            tx.commit().await.unwrap();
            assert_eq!(
                2,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1"
                )
                .bind::<&str>(address.as_ref())
                .fetch_one(&pool)
                .await
                .unwrap()
            );

            let mut tx = uow.begin().await.unwrap();
            let fresh = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-local-new-proof".into(), address.clone(), now)
                .await
                .unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, now)
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::NoRepairNeeded
            ));
            tx.commit().await.unwrap();
            assert_eq!(
                "PENDING",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(fresh.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            assert_eq!(
                3,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1"
                )
                .bind::<&str>(address.as_ref())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn anonymous_grant_race_reuses_email_only_revoke_and_provider_race_repair_stays_anonymous()
         {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let address = email("race-email-only@example.test");
            let now = OffsetDateTime::now_utc();
            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-email-only-proof".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            claim(&worker, &mut tx, original).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let revoke = MarketingConsentCoordinator::new(&mut tx, &repo)
                .email_only_withdrawal(address.clone(), "race-email-only-withdraw".into(), now)
                .await
                .unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, now)
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::ExistingRepair(found) if found == revoke
            ));
            tx.commit().await.unwrap();

            let provider_address = email("race-email-only-provider@example.test");
            let mut tx = uow.begin().await.unwrap();
            let provider_original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in(
                    "race-email-only-provider-proof".into(),
                    provider_address.clone(),
                    now,
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            claim(&worker, &mut tx, provider_original).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .provider_withdrawal(provider_address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let repair = match MarketingConsentCoordinator::new(&mut tx, &repo)
                .repair_raced_grant_if_needed(provider_original, OffsetDateTime::now_utc())
                .await
                .unwrap()
            {
                GrantRaceRepairOutcome::RepairScheduled(id) => id,
                _ => panic!("anonymous provider withdrawal needs an anonymous revoke"),
            };
            let row: (String, String, Option<uuid::Uuid>, Option<i64>, bool, String) = sqlx::query_as(
        "SELECT subject_type, source, user_id, consent_revision, desired, email FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
    ).bind(repair.as_uuid()).fetch_one(tx.connection()).await.unwrap();
            assert_eq!(row.0, "EMAIL_ONLY");
            assert_eq!(row.1, "PROVIDER_RACE_REPAIR");
            assert_eq!(row.2, None);
            assert_eq!(row.3, None);
            assert!(!row.4);
            assert_eq!(row.5, <Email as AsRef<str>>::as_ref(&provider_address));
            tx.commit().await.unwrap();
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn expired_or_identity_blocked_grants_do_not_schedule_race_repair() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let address = email("race-expired@example.test");
            let mut tx = uow.begin().await.unwrap();
            let expired = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in(
                    "race-expired-proof".into(),
                    address.clone(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            sqlx::query("UPDATE marketing_email_consent_sync_intents SET changed_at = changed_at - interval '8 days', not_after = not_after - interval '8 days' WHERE intent_id = $1")
        .bind(expired.as_uuid()).execute(&pool).await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, expired)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(expired, OffsetDateTime::now_utc())
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::NoRepairNeeded
            ));
            tx.commit().await.unwrap();

            let anonymous_address = email("race-registered-later@example.test");
            let mut tx = uow.begin().await.unwrap();
            let anonymous = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in(
                    "race-registered-later-proof".into(),
                    anonymous_address.clone(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let new_owner = UserId::new();
            seed(&pool, new_owner, &anonymous_address).await;
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentIntentWorker::claim_by_id(&worker, &mut tx, anonymous)
                    .await
                    .unwrap(),
                ConsentWorkerClaimOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked)
            ));
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(anonymous, OffsetDateTime::now_utc())
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::NoRepairNeeded
            ));
            tx.commit().await.unwrap();
            assert_eq!(0, sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE source = 'PROVIDER_RACE_REPAIR'"
    ).fetch_one(&pool).await.unwrap());
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn raced_grant_does_not_revoke_a_reassigned_address() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let previous = UserId::new();
            let address = email("race-reassigned@example.test");
            seed(&pool, previous, &address).await;
            let now = OffsetDateTime::now_utc();
            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-reassigned-old".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            claim(&worker, &mut tx, original).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .user_deletion(previous, now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let new_owner = UserId::new();
            seed(&pool, new_owner, &address).await;
            let mut tx = uow.begin().await.unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::NoRepairNeeded
            ));
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let new_grant = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-reassigned-new".into(), address.clone(), now)
                .await
                .unwrap();
            assert!(matches!(
                MarketingConsentCoordinator::new(&mut tx, &repo)
                    .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                    .await
                    .unwrap(),
                GrantRaceRepairOutcome::NoRepairNeeded
            ));
            tx.commit().await.unwrap();
            assert_eq!(
                "PENDING",
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(new_grant.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            assert!(
                sqlx::query_scalar::<_, bool>(
                    "SELECT marketing_email_consent FROM users WHERE user_id = $1"
                )
                .bind(new_owner.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            assert_eq!(0, sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE source = 'PROVIDER_RACE_REPAIR'"
    ).fetch_one(&pool).await.unwrap());
        }

        #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
        async fn race_repair_rolls_back_with_callers_transaction() {
            let pool = get_postgres_client().await;
            let uow = SqlxUnitOfWork::new(pool.clone());
            let repo = SqlxMarketingConsentIntentRepository::new();
            let worker = SqlxMarketingConsentIntentWorker::new();
            let id = UserId::new();
            let address = email("race-repair-rollback@example.test");
            seed(&pool, id, &address).await;
            let now = OffsetDateTime::now_utc();
            let mut tx = uow.begin().await.unwrap();
            let original = MarketingConsentCoordinator::new(&mut tx, &repo)
                .accepted_double_opt_in("race-repair-rollback-proof".into(), address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            claim(&worker, &mut tx, original).await;
            tx.commit().await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            MarketingConsentCoordinator::new(&mut tx, &repo)
                .provider_withdrawal(address.clone(), now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            let before: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1"
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
            let mut tx = uow.begin().await.unwrap();
            let rolled_back = match MarketingConsentCoordinator::new(&mut tx, &repo)
                .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                .await
                .unwrap()
            {
                GrantRaceRepairOutcome::RepairScheduled(id) => id,
                _ => panic!("expected an in-transaction repair"),
            };
            assert_eq!(
                1,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(rolled_back.as_uuid())
                .fetch_one(tx.connection())
                .await
                .unwrap()
            );
            drop(tx);
            assert_eq!(
                0,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
                )
                .bind(rolled_back.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap()
            );
            assert_eq!(before, sqlx::query_as::<_, (bool, i64, i64)>(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1"
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap());
            let mut tx = uow.begin().await.unwrap();
            let scheduled = match MarketingConsentCoordinator::new(&mut tx, &repo)
                .repair_raced_grant_if_needed(original, OffsetDateTime::now_utc())
                .await
                .unwrap()
            {
                GrantRaceRepairOutcome::RepairScheduled(id) => id,
                _ => panic!("rollback must leave the repair schedulable"),
            };
            assert_ne!(rolled_back, scheduled);
            tx.commit().await.unwrap();
        }
    }
}
