//! Connecting the dev entry points to a crawler-local Postgres that may still be starting.
//!
//! A run that just started the Compose container waits for Postgres on this machine, but a server
//! that answers with a definitive refusal, such as an unknown database or failed authentication,
//! will not change its answer: the run fails on the first such answer instead of retrying. A host
//! that does not resolve fails before the first attempt, and a server on another host, which no
//! dev run starts, is waited for only briefly.

use sqlx::postgres::PgConnectOptions;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::Instant;
use tracing::info;

const MAX_ATTEMPTS: u32 = 30;
const INITIAL_DELAY: Duration = Duration::from_millis(200);
const MAX_DELAY: Duration = Duration::from_secs(3);
/// How long a server on another host may refuse or ignore connections before the run fails.
const REMOTE_WAIT: Duration = Duration::from_secs(30);

/// SQLSTATEs a starting or briefly overloaded server answers a connection with
/// (<https://www.postgresql.org/docs/current/errcodes-appendix.html>): `cannot_connect_now` while
/// the server starts up or shuts down, and `too_many_connections`. sqlx itself retries these
/// within one connect attempt (`is_transient_in_connect_phase`); this keeps waiting past it.
const STARTING_SERVER_SQLSTATES: [&str; 2] = ["57P03", "53300"];

/// Why connecting to the crawler-local Postgres failed. Never carries the URL: it holds the
/// database password.
#[derive(Debug, thiserror::Error)]
pub enum PostgresConnectError {
    #[error("the Postgres URL is not a valid connection URL")]
    InvalidUrl(#[source] sqlx::Error),
    #[error("the Postgres host {host} does not resolve")]
    UnknownHost {
        host: String,
        #[source]
        source: io::Error,
    },
    /// The server or the connection settings refused the connection in a way retrying cannot
    /// change, for example `invalid_catalog_name` (`3D000`) or `invalid_password` (`28P01`).
    #[error("Postgres refused the connection permanently on attempt {attempt}")]
    Refused {
        attempt: u32,
        #[source]
        source: sqlx::Error,
    },
    /// The server on this machine did not accept a connection before the attempts ran out. Each
    /// attempt may itself wait up to the pool's acquire timeout, so the wait is longer than the
    /// back-off.
    #[error("Postgres did not accept a connection after {attempts} attempts")]
    NotReady {
        attempts: u32,
        #[source]
        source: sqlx::Error,
    },
    /// The server on another host did not accept a connection within [`REMOTE_WAIT`], for
    /// example because nothing listens on the port or a firewall drops the connection.
    #[error("Postgres at {host}:{port} did not accept a connection within {waited:?}")]
    Unreachable {
        host: String,
        port: u16,
        waited: Duration,
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

/// Where the server a URL names runs, which decides how long a run waits for it to accept
/// connections.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerLocation {
    /// A Unix socket or a host that resolves only to loopback addresses, such as the Compose
    /// container a dev run starts: waited for while it starts.
    Local,
    /// Any other host. No dev run starts it, so a refusal or silence is waited out only for
    /// [`REMOTE_WAIT`].
    Remote { host: String, port: u16 },
}

impl ServerLocation {
    /// Resolves the host `url` names. A host that does not resolve is a wrong URL, not a server
    /// that is still starting.
    async fn of_url(url: &str) -> Result<Self, PostgresConnectError> {
        let options = PgConnectOptions::from_str(url).map_err(PostgresConnectError::InvalidUrl)?;
        // sqlx connects through a Unix socket when the host is a directory path.
        if options.get_socket().is_some() || options.get_host().starts_with('/') {
            return Ok(Self::Local);
        }
        let host = options
            .get_host()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = options.get_port();
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|source| PostgresConnectError::UnknownHost {
                host: host.clone(),
                source,
            })?
            .collect();
        Ok(Self::of_addresses(host, port, &addresses))
    }

    fn of_addresses(host: String, port: u16, addresses: &[SocketAddr]) -> Self {
        if !addresses.is_empty() && addresses.iter().all(|address| address.ip().is_loopback()) {
            Self::Local
        } else {
            Self::Remote { host, port }
        }
    }
}

/// Connects to the Postgres `url` names by running `connect`, which must connect to that URL.
/// Fails at once on a URL whose host does not resolve and on a [`ConnectFailure::Permanent`]
/// error. Otherwise retries with exponential back-off, while a [`ServerLocation::Local`] server
/// starts or for [`REMOTE_WAIT`] on a [`ServerLocation::Remote`] one. Returns the connected value
/// and the attempt that succeeded.
pub async fn connect_with_retry<T, F, Fut>(
    url: &str,
    connect: F,
) -> Result<(T, u32), PostgresConnectError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    let location = ServerLocation::of_url(url).await?;
    retry_while_not_ready(location, connect).await
}

async fn retry_while_not_ready<T, F, Fut>(
    location: ServerLocation,
    mut connect: F,
) -> Result<(T, u32), PostgresConnectError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    let patience = Patience::for_server(location);
    let mut delay = INITIAL_DELAY;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let error = match patience.bound(connect()).await {
            Ok(connected) => return Ok((connected, attempt)),
            Err(error) => error,
        };
        if ConnectFailure::of(&error) == ConnectFailure::Permanent {
            return Err(PostgresConnectError::Refused {
                attempt,
                source: error,
            });
        }
        let error = patience.give_up_before_waiting(attempt, delay, error)?;
        info!(attempt, error = %error, "Postgres not ready yet, retrying…");
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(MAX_DELAY);
    }
}

/// How long [`retry_while_not_ready`] keeps retrying a server that is not ready.
enum Patience {
    /// [`MAX_ATTEMPTS`] attempts, each bounded by the pool's own acquire timeout.
    Attempts,
    /// Attempts until `deadline`, [`REMOTE_WAIT`] after the first one.
    Until {
        deadline: Instant,
        host: String,
        port: u16,
    },
}

impl Patience {
    fn for_server(location: ServerLocation) -> Self {
        match location {
            ServerLocation::Local => Self::Attempts,
            ServerLocation::Remote { host, port } => Self::Until {
                deadline: Instant::now() + REMOTE_WAIT,
                host,
                port,
            },
        }
    }

    /// Cuts an attempt off at the deadline: a host that drops packets would otherwise hold it for
    /// the pool's whole acquire timeout.
    async fn bound<T>(
        &self,
        attempt: impl Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, sqlx::Error> {
        match self {
            Self::Attempts => attempt.await,
            Self::Until { deadline, .. } => tokio::time::timeout_at(*deadline, attempt)
                .await
                .unwrap_or(Err(sqlx::Error::PoolTimedOut)),
        }
    }

    /// Hands `error` back when another attempt may follow after `delay`, and turns it into the
    /// final error otherwise.
    fn give_up_before_waiting(
        &self,
        attempt: u32,
        delay: Duration,
        error: sqlx::Error,
    ) -> Result<sqlx::Error, PostgresConnectError> {
        match self {
            Self::Attempts if attempt >= MAX_ATTEMPTS => Err(PostgresConnectError::NotReady {
                attempts: attempt,
                source: error,
            }),
            Self::Until {
                deadline,
                host,
                port,
            } if Instant::now() + delay >= *deadline => Err(PostgresConnectError::Unreachable {
                host: host.clone(),
                port: *port,
                waited: REMOTE_WAIT,
                source: error,
            }),
            Self::Attempts | Self::Until { .. } => Ok(error),
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
    use std::future;
    use std::net::{Ipv4Addr, Ipv6Addr};

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

        let result = retry_while_not_ready(ServerLocation::Local, || {
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

        let result = retry_while_not_ready(ServerLocation::Local, || {
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

        let result = retry_while_not_ready(ServerLocation::Local, || {
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

    fn remote() -> ServerLocation {
        ServerLocation::Remote {
            host: "db.example.com".to_owned(),
            port: 5432,
        }
    }

    #[rstest]
    #[case::ipv4_loopback(&[SocketAddr::from((Ipv4Addr::LOCALHOST, 5432))])]
    #[case::localhost_on_both_stacks(&[
        SocketAddr::from((Ipv4Addr::LOCALHOST, 5432)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, 5432)),
    ])]
    fn should_treat_a_host_resolving_only_to_loopback_as_local(#[case] addresses: &[SocketAddr]) {
        let location = ServerLocation::of_addresses("localhost".to_owned(), 5432, addresses);

        assert_eq!(location, ServerLocation::Local);
    }

    #[rstest]
    #[case::other_host(&[SocketAddr::from((Ipv4Addr::new(10, 0, 0, 7), 5432))])]
    #[case::loopback_and_other_host(&[
        SocketAddr::from((Ipv4Addr::LOCALHOST, 5432)),
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, 7), 5432)),
    ])]
    #[case::no_address(&[])]
    fn should_treat_any_other_host_as_remote(#[case] addresses: &[SocketAddr]) {
        let location = ServerLocation::of_addresses("db.example.com".to_owned(), 5432, addresses);

        assert_eq!(location, remote());
    }

    #[rstest]
    #[case::localhost("postgres://postgres:secret@localhost:5432/crawler_demo")]
    #[case::ipv4_loopback("postgres://postgres:secret@127.0.0.1:5433/crawler_demo")]
    #[case::ipv6_loopback("postgres://postgres:secret@[::1]:5432/crawler_demo")]
    #[case::unix_socket("postgres://postgres:secret@%2Fvar%2Frun%2Fpostgresql/crawler_demo")]
    #[tokio::test]
    async fn should_locate_the_compose_and_socket_servers_on_this_machine(#[case] url: &str) {
        let location = ServerLocation::of_url(url)
            .await
            .unwrap_or_else(|error| panic!("{url} must be located: {error}"));

        assert_eq!(location, ServerLocation::Local);
    }

    #[tokio::test]
    async fn should_fail_before_connecting_when_the_host_does_not_resolve() {
        let attempts = Cell::new(0);

        let result = connect_with_retry(
            "postgres://postgres:secret@crawler-db.invalid:5432/crawler_demo",
            || {
                attempts.set(attempts.get() + 1);
                async { Err::<(), _>(sqlx::Error::PoolTimedOut) }
            },
        )
        .await;

        let error = result
            .err()
            .unwrap_or_else(|| panic!("an unresolvable host must fail"));
        assert!(
            matches!(&error, PostgresConnectError::UnknownHost { host, .. } if host == "crawler-db.invalid"),
            "{error:?}"
        );
        assert!(!error.to_string().contains("secret"), "{error}");
        assert_eq!(attempts.get(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn should_stop_waiting_for_a_remote_server_that_refuses_connections() {
        let started = Instant::now();

        let result = retry_while_not_ready(remote(), || async {
            Err::<(), _>(sqlx::Error::Io(io::ErrorKind::ConnectionRefused.into()))
        })
        .await;

        assert!(
            matches!(
                result,
                Err(PostgresConnectError::Unreachable {
                    port: 5432,
                    source: sqlx::Error::Io(_),
                    ..
                })
            ),
            "{result:?}"
        );
        assert!(started.elapsed() <= REMOTE_WAIT, "{:?}", started.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn should_cut_off_an_attempt_to_a_remote_server_that_never_answers() {
        let started = Instant::now();

        let result =
            retry_while_not_ready(remote(), future::pending::<Result<(), sqlx::Error>>).await;

        assert!(
            matches!(
                result,
                Err(PostgresConnectError::Unreachable {
                    source: sqlx::Error::PoolTimedOut,
                    ..
                })
            ),
            "{result:?}"
        );
        assert_eq!(started.elapsed(), REMOTE_WAIT);
    }

    #[tokio::test(start_paused = true)]
    async fn should_connect_once_a_starting_remote_server_accepts_connections() {
        let attempts = Cell::new(0);

        let result = retry_while_not_ready(remote(), || {
            attempts.set(attempts.get() + 1);
            let answer = match attempts.get() {
                1 => Err(server_answer("57P03")),
                _ => Ok("pool"),
            };
            async move { answer }
        })
        .await;

        assert!(matches!(result, Ok(("pool", 2))));
    }
}
