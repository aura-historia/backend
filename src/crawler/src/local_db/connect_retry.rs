//! Connecting the dev entry points to a crawler-local Postgres that may still be starting.
//!
//! A run that just started the Compose container waits for Postgres, but a server that answers
//! with a definitive refusal, such as an unknown database or failed authentication, will not
//! change its answer: the run fails on the first such answer instead of retrying for a minute.

use std::future::Future;
use std::time::Duration;
use tracing::info;

const MAX_ATTEMPTS: u32 = 30;
const INITIAL_DELAY: Duration = Duration::from_millis(200);
const MAX_DELAY: Duration = Duration::from_secs(3);

/// SQLSTATEs a starting or briefly overloaded server answers a connection with
/// (<https://www.postgresql.org/docs/current/errcodes-appendix.html>): `cannot_connect_now` while
/// the server starts up or shuts down, and `too_many_connections`. sqlx itself retries these
/// within one connect attempt (`is_transient_in_connect_phase`); this keeps waiting past it.
const STARTING_SERVER_SQLSTATES: [&str; 2] = ["57P03", "53300"];

/// Why connecting to the crawler-local Postgres failed. Never carries the URL: it holds the
/// database password.
#[derive(Debug, thiserror::Error)]
pub enum PostgresConnectError {
    /// The server or the connection settings refused the connection in a way retrying cannot
    /// change, for example `invalid_catalog_name` (`3D000`) or `invalid_password` (`28P01`).
    #[error("Postgres refused the connection permanently on attempt {attempt}")]
    Refused {
        attempt: u32,
        #[source]
        source: sqlx::Error,
    },
    /// The server did not accept a connection before the attempts ran out. Each attempt may
    /// itself wait up to the pool's acquire timeout, so the wait is longer than the back-off.
    #[error("Postgres did not accept a connection after {attempts} attempts")]
    NotReady {
        attempts: u32,
        #[source]
        source: sqlx::Error,
    },
}

/// Whether a failed connect may succeed once Postgres has started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectFailure {
    /// The server is not reachable or not ready yet: no answer, a reset, a timeout, or a
    /// starting-up SQLSTATE.
    NotReady,
    /// The server answered with a definitive refusal, or the settings are invalid.
    Permanent,
}

impl ConnectFailure {
    fn of(error: &sqlx::Error) -> Self {
        match error {
            // The server answered. Only a starting or overloaded server answers with a code that
            // changes on its own; every other code (unknown database, authentication, missing
            // privilege) stays until someone changes the server or the URL.
            sqlx::Error::Database(error) => match error.code() {
                Some(code) if STARTING_SERVER_SQLSTATES.contains(&code.as_ref()) => Self::NotReady,
                _ => Self::Permanent,
            },
            sqlx::Error::Configuration(_) => Self::Permanent,
            // No answer from Postgres yet: refused or reset connections, timeouts and broken
            // handshakes while the container starts.
            _ => Self::NotReady,
        }
    }
}

/// Runs `connect` until it succeeds, retrying with exponential back-off while Postgres is not
/// ready, and fails at once on a [`ConnectFailure::Permanent`] error. Returns the connected
/// value and the attempt that succeeded.
pub async fn connect_with_retry<T, F, Fut>(mut connect: F) -> Result<(T, u32), PostgresConnectError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    let mut delay = INITIAL_DELAY;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let error = match connect().await {
            Ok(connected) => return Ok((connected, attempt)),
            Err(error) => error,
        };
        match ConnectFailure::of(&error) {
            ConnectFailure::Permanent => {
                return Err(PostgresConnectError::Refused {
                    attempt,
                    source: error,
                });
            }
            ConnectFailure::NotReady if attempt == MAX_ATTEMPTS => {
                return Err(PostgresConnectError::NotReady {
                    attempts: attempt,
                    source: error,
                });
            }
            ConnectFailure::NotReady => {
                info!(attempt, error = %error, "Postgres not ready yet, retrying…");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(MAX_DELAY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::borrow::Cow;
    use std::cell::Cell;
    use std::error::Error as StdError;
    use std::io;

    /// A server answer carrying `code`, as `sqlx` reports it for a Postgres error response.
    #[derive(Debug)]
    struct ServerAnswer(&'static str);

    impl std::fmt::Display for ServerAnswer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "server answered {}", self.0)
        }
    }

    impl StdError for ServerAnswer {}

    impl sqlx::error::DatabaseError for ServerAnswer {
        fn message(&self) -> &str {
            "server answer"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.0))
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    fn server_answer(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(ServerAnswer(code)))
    }

    #[rstest]
    #[case::unknown_database(server_answer("3D000"))]
    #[case::wrong_password(server_answer("28P01"))]
    #[case::authorization_rejected(server_answer("28000"))]
    #[case::no_connect_privilege(server_answer("42501"))]
    #[case::invalid_settings(sqlx::Error::Configuration("bad sslmode".into()))]
    fn should_treat_definitive_refusals_as_permanent(#[case] error: sqlx::Error) {
        assert_eq!(ConnectFailure::of(&error), ConnectFailure::Permanent);
    }

    #[rstest]
    #[case::starting_up(server_answer("57P03"))]
    #[case::too_many_connections(server_answer("53300"))]
    #[case::refused(sqlx::Error::Io(io::ErrorKind::ConnectionRefused.into()))]
    #[case::reset_by_container_proxy(sqlx::Error::Io(io::ErrorKind::ConnectionReset.into()))]
    #[case::closed_during_handshake(sqlx::Error::Io(io::ErrorKind::UnexpectedEof.into()))]
    #[case::pool_timed_out(sqlx::Error::PoolTimedOut)]
    fn should_keep_waiting_while_postgres_is_not_ready(#[case] error: sqlx::Error) {
        assert_eq!(ConnectFailure::of(&error), ConnectFailure::NotReady);
    }

    #[tokio::test(start_paused = true)]
    async fn should_fail_on_the_first_attempt_when_the_database_is_unknown() {
        let attempts = Cell::new(0);

        let result = connect_with_retry(|| {
            attempts.set(attempts.get() + 1);
            async { Err::<(), _>(server_answer("3D000")) }
        })
        .await;

        assert!(matches!(
            result,
            Err(PostgresConnectError::Refused { attempt: 1, .. })
        ));
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn should_connect_once_a_starting_postgres_accepts_connections() {
        let attempts = Cell::new(0);

        let result = connect_with_retry(|| {
            attempts.set(attempts.get() + 1);
            let answer = match attempts.get() {
                1 => Err(sqlx::Error::Io(io::ErrorKind::ConnectionRefused.into())),
                2 => Err(server_answer("57P03")),
                _ => Ok("pool"),
            };
            async move { answer }
        })
        .await;

        assert!(matches!(result, Ok(("pool", 3))));
    }

    #[tokio::test(start_paused = true)]
    async fn should_give_up_when_postgres_never_becomes_ready() {
        let attempts = Cell::new(0);

        let result = connect_with_retry(|| {
            attempts.set(attempts.get() + 1);
            async { Err::<(), _>(sqlx::Error::PoolTimedOut) }
        })
        .await;

        assert!(matches!(
            result,
            Err(PostgresConnectError::NotReady {
                attempts: MAX_ATTEMPTS,
                ..
            })
        ));
        assert_eq!(attempts.get(), MAX_ATTEMPTS);
    }
}
