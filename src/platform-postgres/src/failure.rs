use std::{error::Error, fmt};

/// The five-character SQLSTATE that PostgreSQL reported for a failed statement.
///
/// Only the standardized class/condition code is kept. Server messages, details, hints and
/// constraint or column names can repeat persisted values, so they never leave the error chain.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SqlState([u8; 5]);

impl SqlState {
    /// Accepts only the standardized shape: five ASCII digits or upper-case letters.
    pub fn parse(code: &str) -> Option<Self> {
        let code: [u8; 5] = code.as_bytes().try_into().ok()?;
        code.iter()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
            .then_some(Self(code))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl fmt::Debug for SqlState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Display for SqlState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Bounded, loggable facts about the first SQLx failure in an error chain.
///
/// The kind is an allowlisted label and the SQLSTATE a validated code. Neither carries the error
/// text, which can include server messages, query fragments or persisted values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostgresFailure {
    kind: &'static str,
    sqlstate: Option<SqlState>,
}

impl PostgresFailure {
    /// Returns `None` when the chain holds no SQLx error, for example when an adapter rejected a
    /// persisted value it had already read successfully.
    pub fn find(error: &(dyn Error + 'static)) -> Option<Self> {
        std::iter::successors(Some(error), |error| (*error).source())
            .find_map(|error| error.downcast_ref::<sqlx::Error>())
            .map(Self::classify)
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn sqlstate(&self) -> Option<SqlState> {
        self.sqlstate
    }

    fn classify(error: &sqlx::Error) -> Self {
        let kind = match error {
            sqlx::Error::Database(_) => "database",
            sqlx::Error::PoolTimedOut => "pool_timed_out",
            sqlx::Error::PoolClosed => "pool_closed",
            sqlx::Error::Io(_) => "io",
            sqlx::Error::Tls(_) => "tls",
            sqlx::Error::Protocol(_) => "protocol",
            sqlx::Error::RowNotFound => "row_not_found",
            sqlx::Error::TypeNotFound { .. }
            | sqlx::Error::ColumnIndexOutOfBounds { .. }
            | sqlx::Error::ColumnNotFound(_)
            | sqlx::Error::ColumnDecode { .. }
            | sqlx::Error::Decode(_) => "decode",
            sqlx::Error::Encode(_) => "encode",
            sqlx::Error::WorkerCrashed => "worker_crashed",
            sqlx::Error::BeginFailed | sqlx::Error::InvalidSavePointStatement => {
                "transaction_state"
            }
            sqlx::Error::Configuration(_) => "configuration",
            _ => "other",
        };
        let sqlstate = error
            .as_database_error()
            .and_then(|error| error.code())
            .and_then(|code| SqlState::parse(&code));
        Self { kind, sqlstate }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{error::box_error, transaction::TransactionError};
    use sqlx::error::{DatabaseError, ErrorKind};
    use std::borrow::Cow;

    const SENTINEL: &str = "SENTINEL_PERSISTED_VALUE";

    #[derive(Debug, thiserror::Error)]
    #[error("duplicate key value {SENTINEL}")]
    struct FakeDatabaseError {
        code: &'static str,
    }

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &str {
            SENTINEL
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
        }

        fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::UniqueViolation
        }
    }

    fn database_error(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(FakeDatabaseError { code }))
    }

    #[test]
    fn should_find_sqlstate_of_a_database_error_nested_below_a_transaction_error() {
        let error = TransactionError::CommitFailed(box_error(database_error("40001")));

        let failure = PostgresFailure::find(&error);

        assert_eq!(Some("database"), failure.map(|failure| failure.kind()));
        assert_eq!(
            Some("40001"),
            failure
                .and_then(|failure| failure.sqlstate())
                .as_ref()
                .map(SqlState::as_str)
        );
        assert!(!format!("{failure:?}").contains(SENTINEL));
    }

    #[test]
    fn should_classify_connection_failures_without_a_sqlstate() {
        for (error, kind) in [
            (sqlx::Error::PoolTimedOut, "pool_timed_out"),
            (sqlx::Error::Io(std::io::Error::other(SENTINEL)), "io"),
            (sqlx::Error::Protocol(SENTINEL.to_owned()), "protocol"),
        ] {
            let failure = PostgresFailure::find(&error);

            assert_eq!(Some(kind), failure.map(|failure| failure.kind()));
            assert_eq!(None, failure.and_then(|failure| failure.sqlstate()));
        }
    }

    #[test]
    fn should_find_nothing_when_no_sqlx_error_is_in_the_chain() {
        let error = TransactionError::BeginFailed(box_error(std::io::Error::other(SENTINEL)));

        assert_eq!(None, PostgresFailure::find(&error));
    }

    #[test]
    fn should_drop_codes_outside_the_standardized_sqlstate_shape() {
        for code in ["4000", "400001", "40p01", "4000 ", "4000é"] {
            assert_eq!(None, SqlState::parse(code));
        }
        let failure = PostgresFailure::find(&database_error("toolong"));

        assert_eq!(Some("database"), failure.map(|failure| failure.kind()));
        assert_eq!(None, failure.and_then(|failure| failure.sqlstate()));
    }
}
