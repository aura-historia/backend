//! Application command identity. Receipts and their retention belong to the owning use case.
use crate::error::BoxError;
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: String) -> Result<Self, InvalidIdempotencyKey> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
        {
            return Err(InvalidIdempotencyKey);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdempotencyKey(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "idempotency key must contain 1–128 ASCII letters, digits, hyphens, underscores, periods or colons"
)]
pub struct InvalidIdempotencyKey;

/// Hash a versioned, canonical encoding of validated submitted intent, excluding derived state.
#[derive(Clone, PartialEq, Eq)]
pub struct CommandFingerprint([u8; 32]);

impl CommandFingerprint {
    pub fn digest(version: u32, intent: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(version.to_be_bytes());
        digest.update(intent);
        Self(digest.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for CommandFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CommandFingerprint(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CommandReceiptError {
    #[error("idempotency key is bound to different command intent")]
    Mismatch,
    #[error("command with this idempotency key is in progress")]
    InProgress,
    #[error("command result cannot be recovered or confirmed")]
    ResultUnavailable,
    #[error("command receipt storage failed")]
    Storage(#[source] BoxError),
    #[error("persisted command receipt is invalid")]
    InvalidPersistedState(#[source] BoxError),
}

impl CommandReceiptError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Mismatch => "mismatch",
            Self::InProgress => "in_progress",
            Self::ResultUnavailable => "result_unavailable",
            Self::Storage(_) => "storage_failure",
            Self::InvalidPersistedState(_) => "invalid_evidence",
        }
    }
}
pub fn observe(operation: &'static str, error: CommandReceiptError) -> CommandReceiptError {
    tracing::info!(
        event = "command.retry",
        operation,
        outcome = error.category()
    );
    error
}

/// Transaction-bound successful-command evidence, with a use-case-specific outcome codec.
/// Lookup never reserves a key. Coordinate must serialize duplicates until transaction end.
#[async_trait::async_trait]
pub trait CommandReceipts<Intent, Outcome>: Send {
    async fn find(&mut self, intent: &Intent) -> Result<Option<Outcome>, CommandReceiptError>;
    async fn coordinate(&mut self, intent: &Intent) -> Result<(), CommandReceiptError>;
    async fn complete(
        &mut self,
        intent: &Intent,
        outcome: &Outcome,
    ) -> Result<(), CommandReceiptError>;
}

pub trait CommandReceiptsFactory<Tx, Intent, Outcome>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl CommandReceipts<Intent, Outcome> + 'tx;
}

/// Validated intent owned by a logical use case; transport aliases share its operation name.
/// Private owner receipts are erased with that account. Other fences survive target deletion.
#[derive(Clone)]
pub struct CommandIntent {
    pub actor: String,
    pub operation: &'static str,
    pub target: String,
    pub private_owner: Option<user_core::user_id::UserId>,
    pub key: IdempotencyKey,
    pub fingerprint: CommandFingerprint,
}

/// Private resource identity whose linked response content must be erased together.
pub struct CommandResource {
    pub owner: user_core::user_id::UserId,
    pub target: String,
}

impl CommandIntent {
    pub fn new(
        context: &crate::operation_context::OperationContext,
        operation: &'static str,
        target: impl Into<String>,
        private_owner: Option<user_core::user_id::UserId>,
        mut intent: serde_json::Value,
    ) -> Option<Self> {
        use crate::operation_context::Principal;
        let key = context.command_key.clone()?;
        let actor = match &context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => {
                format!("USER:{id}")
            }
            Principal::Service(id) => format!("SERVICE:{id}"),
            Principal::System => "SYSTEM".into(),
            Principal::Anonymous => return None,
        };
        intent.sort_all_objects();
        Some(Self {
            actor,
            operation,
            target: target.into(),
            private_owner,
            key,
            fingerprint: CommandFingerprint::digest(1, intent.to_string().as_bytes()),
        })
    }
}

/// Version 1 application outcome codec. Implementations explicitly map validated application
/// values; they must never encode transport responses or reusable credential material.
pub trait CommandOutcome: Sized {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        None
    }
    fn encode(&self) -> Result<serde_json::Value, BoxError>;
    fn decode(value: serde_json::Value) -> Result<Self, BoxError>;
}

impl CommandOutcome for () {
    fn encode(&self) -> Result<serde_json::Value, BoxError> {
        Ok(serde_json::Value::Null)
    }
    fn decode(value: serde_json::Value) -> Result<Self, BoxError> {
        if value.is_null() {
            Ok(())
        } else {
            Err(crate::error::static_error("invalid empty command outcome"))
        }
    }
}

pub async fn replay<T: CommandOutcome, Tx: crate::transaction::Transaction>(
    tx: &mut Tx,
    intent: Option<&CommandIntent>,
) -> Result<Option<T>, CommandReceiptError> {
    let Some(intent) = intent else {
        return Ok(None);
    };
    let Some(value) = tx
        .command_receipt(intent)
        .await
        .map_err(|e| observe(intent.operation, e))?
    else {
        return Ok(None);
    };
    let result = T::decode(value).map_err(CommandReceiptError::InvalidPersistedState)?;
    if let (Some(expected), Some(actual)) = (intent.private_owner, result.private_owner())
        && expected != actual
    {
        return Err(CommandReceiptError::InvalidPersistedState(
            crate::error::static_error("command outcome owner mismatch"),
        ));
    }

    tracing::info!(
        event = "command.replayed",
        operation = intent.operation,
        outcome = "success"
    );
    Ok(Some(result))
}

pub async fn complete<T: CommandOutcome, Tx: crate::transaction::Transaction>(
    tx: &mut Tx,
    intent: Option<&CommandIntent>,
    outcome: &T,
) -> Result<(), CommandReceiptError> {
    if let Some(intent) = intent {
        let mut intent = intent.clone();
        intent.private_owner = outcome.private_owner().or(intent.private_owner);
        let outcome = outcome
            .encode()
            .map_err(CommandReceiptError::InvalidPersistedState)?;
        tx.complete_command(&intent, outcome)
            .await
            .map_err(|e| observe(intent.operation, e))?;
    }
    Ok(())
}

pub fn field<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    name: &str,
) -> Result<T, BoxError> {
    let field = value
        .get(name)
        .ok_or_else(|| crate::error::static_error("missing command outcome field"))?;
    serde_json::from_value(field.clone()).map_err(crate::error::box_error)
}

pub fn timestamp(value: &serde_json::Value, name: &str) -> Result<time::OffsetDateTime, BoxError> {
    time::OffsetDateTime::parse(
        &field::<String>(value, name)?,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(crate::error::box_error)
}

pub fn time_value(value: time::OffsetDateTime) -> Result<serde_json::Value, BoxError> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map(serde_json::Value::String)
        .map_err(crate::error::box_error)
}

pub fn patch<T>(
    value: &crate::patch_field::PatchField<T>,
    encode: impl FnOnce(&T) -> serde_json::Value,
) -> serde_json::Value {
    match value {
        crate::patch_field::PatchField::Unchanged => serde_json::json!({"kind":"UNCHANGED"}),
        crate::patch_field::PatchField::Clear => serde_json::json!({"kind":"CLEAR"}),
        crate::patch_field::PatchField::Set(value) => {
            serde_json::json!({"kind":"SET","value":encode(value)})
        }
    }
}

/// Field-only metadata outcomes; fields deserialize through their existing validated newtypes.
#[macro_export]
macro_rules! command_outcome {
    ($ty:ty { $($field:ident),+ $(,)? }) => {
        impl $crate::idempotency::CommandOutcome for $ty {
            fn encode(&self)->Result<serde_json::Value,$crate::error::BoxError>{Ok(serde_json::json!({$(stringify!($field):self.$field),+}))}
            fn decode(value:serde_json::Value)->Result<Self,$crate::error::BoxError>{Ok(Self{$($field:$crate::idempotency::field(&value,stringify!($field))?),+})}
        }
    };
    (unit $ty:ident) => {
        impl $crate::idempotency::CommandOutcome for $ty {
            fn encode(&self)->Result<serde_json::Value,$crate::error::BoxError>{Ok(serde_json::Value::Null)}
            fn decode(value:serde_json::Value)->Result<Self,$crate::error::BoxError>{
                <() as $crate::idempotency::CommandOutcome>::decode(value).map(|()|Self)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_policy_is_bounded_and_does_not_log_keys() {
        for key in ["", "two keys,one", " key", "key ", "ümlaut", "a/b", "a\nb"] {
            assert!(IdempotencyKey::new(key.into()).is_err());
        }
        assert!(IdempotencyKey::new("a".repeat(129)).is_err());
        assert!(IdempotencyKey::new("0ABC-_.:z".into()).is_ok());
        let key = IdempotencyKey::new("a".repeat(128)).unwrap();
        assert!(!format!("{key:?}").contains(key.as_str()));
        assert!(IdempotencyKey::new("command-123_.:".into()).is_ok());
    }

    #[test]
    fn fingerprints_bind_the_codec_version_and_submitted_intent() {
        let one = CommandFingerprint::digest(1, b"intent");
        assert_eq!(one, CommandFingerprint::digest(1, b"intent"));
        assert_ne!(one, CommandFingerprint::digest(2, b"intent"));
        assert_ne!(one, CommandFingerprint::digest(1, b"other"));
    }
}
