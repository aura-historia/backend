use crate::state::ReadinessCheck;
use async_trait::async_trait;
use opensearch::{OpenSearch, SearchParts};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

pub(crate) async fn health() -> &'static str {
    "ok\n"
}

const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn ready(
    axum::extract::State(readiness): axum::extract::State<Arc<dyn ReadinessCheck>>,
) -> axum::http::StatusCode {
    match tokio::time::timeout(READINESS_CHECK_TIMEOUT, readiness.check()).await {
        Ok(Ok(())) => axum::http::StatusCode::NO_CONTENT,
        _ => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(crate) struct RuntimeReadiness {
    postgres: PgPool,
    opensearch: OpenSearch,
}

impl RuntimeReadiness {
    pub(crate) fn new(postgres: PgPool, opensearch: OpenSearch) -> Self {
        Self {
            postgres,
            opensearch,
        }
    }
}

#[derive(serde::Deserialize)]
struct ReadinessSearchResponse {
    timed_out: bool,
    #[serde(rename = "_shards")]
    shards: ReadinessShardStats,
    hits: ReadinessHits,
    error: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct ReadinessShardStats {
    total: u64,
    successful: u64,
    failed: u64,
}

#[derive(serde::Deserialize)]
struct ReadinessHits {
    total: ReadinessHitCount,
    hits: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct ReadinessHitCount {
    #[serde(rename = "value")]
    _value: u64,
    relation: String,
}

#[async_trait]
impl ReadinessCheck for RuntimeReadiness {
    async fn check(&self) -> Result<(), ()> {
        let value: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&self.postgres)
            .await
            .map_err(|_| ())?;
        if value != 1 {
            return Err(());
        }
        let response = self
            .opensearch
            .search(SearchParts::Index(&["product-listings"]))
            .body(serde_json::json!({ "size": 0, "query": { "match_all": {} } }))
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status_code().is_success() {
            return Err(());
        }
        let search: ReadinessSearchResponse = response.json().await.map_err(|_| ())?;
        if search.timed_out
            || search.error.is_some()
            || search.shards.total == 0
            || search.shards.successful == 0
            || search.shards.failed != 0
            || !matches!(search.hits.total.relation.as_str(), "eq" | "gte")
            || !search.hits.hits.is_empty()
        {
            return Err(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app, state::AppState};
    use axum::Router;
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use opensearch::http::transport::TransportBuilder;
    use opensearch::{auth::Credentials, http::transport::SingleNodeConnectionPool};
    use sqlx::postgres::PgPoolOptions;
    use test_api::IntegrationTestService;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new_schema_once("migrations");
    const VALID_SEARCH_RESPONSE: &str = r#"{"took":1,"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"max_score":null,"hits":[]}}"#;

    async fn mock_search(
        status: Option<StatusCode>,
        body: &'static str,
    ) -> (OpenSearch, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local search stub");
        let endpoint =
            url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
            .auth(Credentials::Basic("reader".into(), "secret".into()))
            .build()
            .unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0, "search request closed early");
                request.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&request);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .expect("search must include content length");
                    if body.len() >= content_length {
                        break;
                    }
                }
            }
            if let Some(status) = status {
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status.as_u16(),
                    status.canonical_reason().unwrap(),
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            } else {
                std::future::pending::<()>().await;
            }
            String::from_utf8(request).unwrap()
        });
        (OpenSearch::new(transport), server)
    }

    fn readiness_app(postgres: PgPool, opensearch: OpenSearch) -> Router {
        app(AppState::new().with_readiness(Arc::new(RuntimeReadiness {
            postgres,
            opensearch,
        })))
    }

    async fn get_status(app: Router, path: &str) -> StatusCode {
        get_response(app, path).await.status()
    }

    async fn get_response(app: Router, path: &str) -> axum::response::Response {
        app.oneshot(Request::get(path).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn assert_reader_search(request: &str) {
        assert!(request.starts_with("POST /product-listings/_search HTTP/1.1"));
        assert!(request.contains("\"size\":0"));
        assert!(request.contains("\"match_all\":{}"));
        let credentials = base64::engine::general_purpose::STANDARD.encode("reader:secret");
        assert!(request.to_ascii_lowercase().contains(&format!(
            "authorization: basic {}",
            credentials.to_ascii_lowercase()
        )));
    }

    #[test_api::aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
    async fn readiness_requires_successful_reader_search_and_postgres_query() {
        let postgres = test_api::get_postgres_client().await;
        for (upstream_status, body, expected) in [
            (
                StatusCode::OK,
                VALID_SEARCH_RESPONSE,
                StatusCode::NO_CONTENT,
            ),
            (
                StatusCode::UNAUTHORIZED,
                r#"{"error":"unauthorized"}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::FORBIDDEN,
                r#"{"error":"forbidden"}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"upstream failed"}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (StatusCode::OK, "{}", StatusCode::SERVICE_UNAVAILABLE),
            (StatusCode::OK, "{", StatusCode::SERVICE_UNAVAILABLE),
            (
                StatusCode::OK,
                r#"{"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"hits":[]},"error":{"type":"search_phase_execution_exception"}}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::OK,
                r#"{"timed_out":true,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"hits":[]}}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::OK,
                r#"{"timed_out":false,"_shards":{"total":2,"successful":1,"skipped":0,"failed":1},"hits":{"total":{"value":0,"relation":"eq"},"hits":[]}}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::OK,
                r#"{"timed_out":false,"_shards":{"total":1,"successful":0,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"hits":[]}}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::OK,
                r#"{"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"unexpected"},"hits":[]}}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                StatusCode::OK,
                r#"{"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"hits":[{}]}}"#,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ] {
            let (opensearch, server) = mock_search(Some(upstream_status), body).await;
            let app = readiness_app(postgres.clone(), opensearch);
            let response = get_response(app.clone(), "/v1/ready").await;
            assert_eq!(response.status(), expected);
            if expected == StatusCode::SERVICE_UNAVAILABLE {
                assert!(
                    axum::body::to_bytes(response.into_body(), usize::MAX)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
            assert_eq!(get_status(app, "/v1/health").await, StatusCode::OK);
            assert_reader_search(&server.await.unwrap());
        }

        let (opensearch, server) = mock_search(None, "").await;
        let app = readiness_app(postgres, opensearch);
        let started = tokio::time::Instant::now();
        assert_eq!(
            get_status(app, "/v1/ready").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(started.elapsed() >= READINESS_CHECK_TIMEOUT);
        assert!(started.elapsed() < crate::transport::LAMBDA_REQUEST_TIMEOUT);
        server.abort();
    }

    #[tokio::test]
    async fn readiness_fails_when_postgres_query_cannot_run_but_health_stays_live() {
        let postgres = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://test:test@127.0.0.1:1/test")
            .unwrap();
        let (opensearch, server) = mock_search(Some(StatusCode::OK), VALID_SEARCH_RESPONSE).await;
        let app = readiness_app(postgres, opensearch);
        assert_eq!(
            get_status(app.clone(), "/v1/ready").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(get_status(app, "/v1/health").await, StatusCode::OK);
        server.abort();
    }

    #[tokio::test]
    async fn versioned_probe_paths_are_canonical_and_legacy_paths_are_not_routed() {
        let app = app(AppState::new());
        let health = app
            .clone()
            .oneshot(
                Request::get("/v1/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert!(
            health.headers()[axum::http::header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
        assert_eq!(
            axum::body::to_bytes(health.into_body(), usize::MAX)
                .await
                .unwrap(),
            "ok\n"
        );
        assert_eq!(
            get_status(app.clone(), "/v1/ready").await,
            StatusCode::NO_CONTENT
        );
        for path in [
            "/api/v1/health",
            "/api/v1/ready",
            "/api/v1/health/",
            "/health",
            "/ready",
        ] {
            let response = get_response(app.clone(), path).await;
            assert_eq!(StatusCode::NOT_FOUND, response.status(), "{path}");
            assert!(
                !response
                    .headers()
                    .contains_key(axum::http::header::LOCATION),
                "{path}"
            );
        }
    }
}
