use super::*;
use axum::http::{Request, StatusCode};
use base64::Engine;
use opensearch::http::transport::TransportBuilder;
use sqlx::postgres::PgPoolOptions;
use test_api::IntegrationTestService;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    let endpoint = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
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
    app.oneshot(Request::get(path).body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
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

#[test_api::aura_integration_test(services = [BUSINESS_SCHEMA])]
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
            r#"{"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"hits":[{}]}}"#,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (opensearch, server) = mock_search(Some(upstream_status), body).await;
        let app = readiness_app(postgres.clone(), opensearch);
        assert_eq!(get_status(app.clone(), "/ready").await, expected);
        assert_eq!(get_status(app, "/health").await, StatusCode::OK);
        assert_reader_search(&server.await.unwrap());
    }

    let (opensearch, server) = mock_search(None, "").await;
    let app = readiness_app(postgres, opensearch);
    let started = tokio::time::Instant::now();
    assert_eq!(
        get_status(app, "/ready").await,
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
        get_status(app.clone(), "/ready").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(get_status(app, "/health").await, StatusCode::OK);
    server.abort();
}
