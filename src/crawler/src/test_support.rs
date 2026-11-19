//! Real SQLx crawler migrations on an owned, initially empty database.

use crate::runtime::apply_local_migrations;
use test_api::{IntegrationTestService, Postgres, get_postgres_client};

pub(crate) struct CrawlerDatabase;

#[async_trait::async_trait]
impl IntegrationTestService for CrawlerDatabase {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        // Keep the actual SQLx migrator and ledger; raw schema SQL is not equivalent.
        Postgres::new("src/crawler/tests/fixtures/empty_database")
            .set_up()
            .await;
        let pool = get_postgres_client().await;
        apply_local_migrations(&pool).await;
    }
}

/// Gives `listing_source_id` the crawl root `presentation_url` derives, as a scope refresh that
/// holds only this source does: a changed root moves the source and clears its crawl state and
/// frontier (`docs/crawler/README.md#listingsource-scope`).
pub(crate) async fn refresh_scope_with_crawl_root(
    pool: &sqlx::PgPool,
    listing_source_id: listing_source_core::ListingSourceId,
    presentation_url: &str,
) {
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
        QualificationGate, ScopedListingSource,
    };
    let crawl_root = crate::CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"));
    ListingSourceRegistrationRepositoryImpl::new(pool.clone(), QualificationGate::Off)
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id,
            listing_source_name: listing_source_core::ListingSourceName::try_from("Test source")
                .unwrap_or_else(|error| panic!("valid test name: {error}")),
            listing_source_slug: listing_source_core::ListingSourceSlugId::raw("test-source")
                .unwrap_or_else(|error| panic!("valid test slug: {error}")),
            crawl_enabled: true,
            crawl_root: Some(crawl_root),
        }])
        .await
        .unwrap_or_else(|error| panic!("apply the scope snapshot: {error}"));
}

/// Stores the crawl root `presentation_url` derives on an existing ListingSource row, without
/// the move a scope refresh applies, for a test that only needs a crawlable source.
pub(crate) async fn store_crawl_root(
    pool: &sqlx::PgPool,
    listing_source_id: listing_source_core::ListingSourceId,
    presentation_url: &str,
) {
    let crawl_root = crate::CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"));
    sqlx::query(
        "UPDATE listing_sources SET crawl_root_url = $2, crawl_host = $3 \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(crawl_root.url().as_str())
    .bind(crawl_root.host())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("store the crawl root: {error}"));
}

/// Captures the level and fields of the tracing events logged while its guard is the thread's
/// default subscriber. Use it from tests that assert on log fields.
pub(crate) mod logged_events {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, Once};
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Debug, Clone)]
    pub(crate) struct LoggedEvent {
        pub(crate) level: Level,
        pub(crate) fields: BTreeMap<String, String>,
    }

    impl LoggedEvent {
        pub(crate) fn field(&self, name: &str) -> Option<&str> {
            self.fields.get(name).map(String::as_str)
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct LoggedEvents(Arc<Mutex<Vec<LoggedEvent>>>);

    impl LoggedEvents {
        /// Starts capturing the events logged on this thread until the guard drops. Events
        /// logged on other threads are not recorded, so a capturing test drives the code under
        /// test on a current-thread runtime, the `#[tokio::test]` default.
        pub(crate) fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
            keep_every_callsite_interested();
            let events = Self::default();
            // Registering the new default rebuilds every cached callsite interest.
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::registry().with(events.clone()),
            );
            (events, guard)
        }

        /// The events recorded so far, which are forgotten.
        pub(crate) fn take(&self) -> Vec<LoggedEvent> {
            std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
        }

        pub(crate) fn all(&self) -> Vec<LoggedEvent> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        /// The events that carry `event` as their `event` field.
        pub(crate) fn named(&self, event: &str) -> Vec<LoggedEvent> {
            self.all()
                .into_iter()
                .filter(|logged| logged.field("event") == Some(event))
                .collect()
        }
    }

    /// `tracing` caches each callsite's interest once for the whole process. While a single
    /// dispatcher is alive, the thread that reaches a callsite first decides that interest from
    /// its own default alone: a parallel test thread without a subscriber caches "never", and a
    /// capture on another thread then misses the callsite's events. A global subscriber that
    /// wants every event and records nothing is the default of every thread without its own,
    /// so no cached interest is ever "never" and each event asks its thread's default.
    fn keep_every_callsite_interested() {
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            tracing::subscriber::set_global_default(tracing_subscriber::registry()).unwrap_or_else(
                |error| panic!("crawler unit tests set no global subscriber: {error}"),
            );
        });
    }

    struct Fields(BTreeMap<String, String>);

    impl tracing::field::Visit for Fields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LoggedEvents {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut fields = Fields(BTreeMap::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(LoggedEvent {
                    level: *event.metadata().level(),
                    fields: fields.0,
                });
        }
    }
}

/// A local TLS peer for crawler client tests: the untrusted `localhost` certificate under
/// `network/testdata`, and a client configuration that accepts it.
pub(crate) mod local_tls {
    use std::sync::Arc;

    /// A server configuration presenting the untrusted `localhost` certificate.
    pub(crate) fn server_config() -> Arc<rustls::ServerConfig> {
        let certificate = rustls::pki_types::CertificateDer::from(
            include_bytes!("network/testdata/untrusted-localhost-cert.der").to_vec(),
        );
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
            include_bytes!("network/testdata/untrusted-localhost-key.der").to_vec(),
        );
        Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key.into())
            .unwrap(),
        )
    }

    /// Serves `connections` connections over TLS, as `localhost`. Each request is answered with
    /// `response`, then the TCP connection is closed without TLS `close_notify`, the way some
    /// shops end a body. Answers the server's address.
    pub(crate) fn serve_without_close_notify(
        response: Vec<u8>,
        connections: usize,
    ) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..connections {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                answer_without_close_notify(&mut socket, &response);
            }
        });
        addr
    }

    fn answer_without_close_notify(socket: &mut std::net::TcpStream, response: &[u8]) {
        use std::io::{Read, Write};
        let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut connection = rustls::ServerConnection::new(server_config()).unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            if connection.complete_io(socket).is_err() {
                return;
            }
            match connection.reader().read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => request.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
        connection.writer().write_all(response).unwrap();
        while connection.wants_write() {
            if connection.write_tls(socket).is_err() {
                return;
            }
        }
        // A TCP close, with no `close_notify` alert before it; the client closes its side once
        // it has read up to the close.
        let _ = socket.shutdown(std::net::Shutdown::Write);
        let _ = socket.read_to_end(&mut Vec::new());
    }

    /// A client configuration that accepts any server certificate, since a test server
    /// certificate is no trust anchor.
    pub(crate) fn client_config() -> rustls::ClientConfig {
        let provider = rustls::crypto::ring::default_provider();
        let algorithms = provider.signature_verification_algorithms;
        rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(algorithms)))
            .with_no_client_auth()
    }

    #[derive(Debug)]
    struct AcceptAnyCertificate(rustls::crypto::WebPkiSupportedAlgorithms);

    impl rustls::client::danger::ServerCertVerifier for AcceptAnyCertificate {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            certificate: &rustls::pki_types::CertificateDer<'_>,
            signature: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, certificate, signature, &self.0)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            certificate: &rustls::pki_types::CertificateDer<'_>,
            signature: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, certificate, signature, &self.0)
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.supported_schemes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::logged_events::LoggedEvents;

    fn log_probe() {
        tracing::info!(event = "interest_probe");
    }

    #[test]
    fn should_capture_a_callsite_that_a_thread_without_a_subscriber_reached_first() {
        let (events, _guard) = LoggedEvents::capture();

        std::thread::spawn(log_probe)
            .join()
            .expect("the probe thread finishes");
        log_probe();

        assert_eq!(events.named("interest_probe").len(), 1);
    }
}
