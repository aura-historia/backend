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
    ConsentIntent as PortConsentIntent, ConsentIntentSource as PortSource,
    ConsentSubject as PortSubject, ConsentUser, MarketingConsentIntentError,
    MarketingConsentIntents, MarketingConsentIntentsFactory, NewsletterConfirmationChallengeError,
    NewsletterProfile, UserStorageVersion, marketing_consent_recipient_key,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentIntentSource {
    CognitoSignup,
    AuraDoubleOptIn,
    UserWithdrawal,
    UserDeletion,
    EmailOnlyWithdrawal,
    ProviderRaceRepair,
}

impl ConsentIntentSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::CognitoSignup => "COGNITO_SIGNUP",
            Self::AuraDoubleOptIn => "AURA_DOUBLE_OPT_IN",
            Self::UserWithdrawal => "USER_WITHDRAWAL",
            Self::UserDeletion => "USER_DELETION",
            Self::EmailOnlyWithdrawal => "EMAIL_ONLY_WITHDRAWAL",
            Self::ProviderRaceRepair => "PROVIDER_RACE_REPAIR",
        }
    }

    fn parse(value: &str) -> Result<Self, MarketingConsentPersistenceError> {
        match value {
            "COGNITO_SIGNUP" => Ok(Self::CognitoSignup),
            "AURA_DOUBLE_OPT_IN" => Ok(Self::AuraDoubleOptIn),
            "USER_WITHDRAWAL" => Ok(Self::UserWithdrawal),
            "USER_DELETION" => Ok(Self::UserDeletion),
            "EMAIL_ONLY_WITHDRAWAL" => Ok(Self::EmailOnlyWithdrawal),
            "PROVIDER_RACE_REPAIR" => Ok(Self::ProviderRaceRepair),
            _ => Err(MarketingConsentPersistenceError::InvalidPersistedState),
        }
    }

    fn accepts(self, subject: ConsentSubject, desired: bool) -> bool {
        matches!(
            (subject, desired, self),
            (
                ConsentSubject::User(_),
                true,
                Self::CognitoSignup | Self::AuraDoubleOptIn
            ) | (
                ConsentSubject::User(_),
                false,
                Self::UserWithdrawal | Self::UserDeletion | Self::ProviderRaceRepair
            ) | (ConsentSubject::EmailOnly, true, Self::AuraDoubleOptIn)
                | (
                    ConsentSubject::EmailOnly,
                    false,
                    Self::EmailOnlyWithdrawal | Self::ProviderRaceRepair
                )
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentSubject {
    User(UserId),
    EmailOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentIntentStatus {
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
pub struct MarketingConsentIntent {
    pub intent_id: MarketingConsentSyncIntentId,
    pub source_key: String,
    pub subject: ConsentSubject,
    pub source: ConsentIntentSource,
    pub email: Email,
    pub profile_snapshot: Option<NewsletterProfile>,
    pub recipient_key: String,
    pub desired: bool,
    pub consent_revision: Option<i64>,
    pub changed_at: OffsetDateTime,
    pub status: ConsentIntentStatus,
    pub not_after: Option<OffsetDateTime>,
}

#[derive(Clone)]
pub struct MarketingConsentIntentClaim {
    pub intent: MarketingConsentIntent,
    pub lease_token: Uuid,
    pub lease_expires_at: OffsetDateTime,
    pub attempt_count: i32,
    pub prior_attempt_write_ambiguous: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentIntentFinalization<'a> {
    Applied {
        provider_contact_id: Option<&'a str>,
    },
    Failed {
        error_code: &'a str,
    },
    Blocked {
        error_code: &'a str,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum MarketingConsentPersistenceError {
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
        let source = ConsentIntentSource::parse(&self.source)?;
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
        .bind(source.as_str())
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
    pub async fn find_by_source_key(
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
    pub async fn record_user_transition(
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
    pub async fn record_user_transition_with_profile(
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
        let revision: i64 = sqlx::query_scalar("UPDATE users SET marketing_email_consent = $2, marketing_email_consent_revision = marketing_email_consent_revision + 1, marketing_email_consent_changed_at = $3, version = version + 1, updated = now() WHERE user_id = $1 AND email = $4 AND version = $5 RETURNING marketing_email_consent_revision")
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

    pub async fn record_email_only_intent(
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
    pub async fn record_email_only_intent_with_profile(
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
    pub async fn cancel_provider_backsync(
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
    pub async fn repair_raced_grant_if_needed(
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

        // Fetch the reason separately so the standard intent decoder remains the
        // single validation path for all immutable target fields.
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
        let intent = row.into_intent()?;
        if !intent.desired || attempts == 0 || finalized {
            return Ok(GrantRaceRepairOutcome::NoRepairNeeded);
        }
        let reason: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT last_error_code FROM {TABLE} WHERE intent_id = $1"
        )))
        .bind(original.as_uuid())
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        let provider_race = intent.status == ConsentIntentStatus::Blocked
            && reason.as_deref() == Some("PROVIDER_WITHDRAWAL_RACE_CANDIDATE");
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
    pub async fn record_user_deletion(
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

impl From<PortSource> for ConsentIntentSource {
    fn from(source: PortSource) -> Self {
        match source {
            PortSource::CognitoSignup => Self::CognitoSignup,
            PortSource::AuraDoubleOptIn => Self::AuraDoubleOptIn,
            PortSource::UserWithdrawal => Self::UserWithdrawal,
            PortSource::UserDeletion => Self::UserDeletion,
            PortSource::EmailOnlyWithdrawal => Self::EmailOnlyWithdrawal,
            PortSource::ProviderRaceRepair => Self::ProviderRaceRepair,
        }
    }
}

impl From<MarketingConsentIntent> for PortConsentIntent {
    fn from(intent: MarketingConsentIntent) -> Self {
        Self {
            intent_id: intent.intent_id,
            source_key: intent.source_key,
            subject: match intent.subject {
                ConsentSubject::User(id) => PortSubject::User(id),
                ConsentSubject::EmailOnly => PortSubject::EmailOnly,
            },
            source: match intent.source {
                ConsentIntentSource::CognitoSignup => PortSource::CognitoSignup,
                ConsentIntentSource::AuraDoubleOptIn => PortSource::AuraDoubleOptIn,
                ConsentIntentSource::UserWithdrawal => PortSource::UserWithdrawal,
                ConsentIntentSource::UserDeletion => PortSource::UserDeletion,
                ConsentIntentSource::EmailOnlyWithdrawal => PortSource::EmailOnlyWithdrawal,
                ConsentIntentSource::ProviderRaceRepair => PortSource::ProviderRaceRepair,
            },
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
            "SELECT user_id, email, version FROM users WHERE user_id = $1",
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
            "SELECT user_id, email, version FROM users WHERE email = $1",
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
        source: PortSource,
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
                source.into(),
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
        source: PortSource,
        source_key: &str,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<PortConsentIntent, MarketingConsentIntentError> {
        Ok(SqlxMarketingConsentIntentRepository::new()
            .record_email_only_intent_with_profile(
                self.tx,
                email,
                desired,
                source.into(),
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
        let updated = sqlx::query("UPDATE users SET marketing_email_consent = false, marketing_email_consent_revision = marketing_email_consent_revision + 1, marketing_email_consent_changed_at = $3, version = version + 1, updated = now() WHERE user_id = $1 AND email = $2 AND version = $4 AND marketing_email_consent = true")
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
    pub async fn claim_by_id(
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
    pub async fn read_claim(
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

    pub async fn finalize(
        &self,
        tx: &mut SqlxTransaction,
        claim: &MarketingConsentIntentClaim,
        result: ConsentIntentFinalization<'_>,
        completed_at: OffsetDateTime,
    ) -> Result<bool, MarketingConsentPersistenceError> {
        let (status, contact_id, error_code) = match result {
            ConsentIntentFinalization::Applied {
                provider_contact_id,
            } => (ConsentIntentStatus::Applied, provider_contact_id, None),
            ConsentIntentFinalization::Failed { error_code } => {
                (ConsentIntentStatus::Failed, None, Some(error_code))
            }
            ConsentIntentFinalization::Blocked { error_code } => {
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
            .bind(claim.intent.source.as_str())
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
    pub async fn release_for_retry(
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
            subject: match intent.subject {
                PortSubject::User(id) => ConsentSubject::User(id),
                PortSubject::EmailOnly => ConsentSubject::EmailOnly,
            },
            source: intent.source.into(),
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
        let result = match result {
            ConsentWorkerFinalization::Applied {
                provider_contact_id,
            } => ConsentIntentFinalization::Applied {
                provider_contact_id,
            },
            ConsentWorkerFinalization::Failed { error_code } => {
                ConsentIntentFinalization::Failed { error_code }
            }
            ConsentWorkerFinalization::Blocked { error_code } => {
                ConsentIntentFinalization::Blocked { error_code }
            }
        };
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
}
