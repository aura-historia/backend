use crate::{
    auth::{TransportPrincipal, request_metadata},
    error::ApiError,
    state::PublicOverviewState,
    transport::cache,
};
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use product_listing_service::use_cases::PublicCatalogueOverview;
use serde::Serialize;
use time::OffsetDateTime;

const PUBLIC_OVERVIEW_SCHEMA_VERSION: u8 = 1;
/// Matches the dedicated CloudFront overview policy ceiling; other routes keep their own TTLs.
pub(crate) const PUBLIC_OVERVIEW_SHARED_TTL_SECONDS: u32 = 21_600;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicOverviewData {
    schema_version: u8,
    #[serde(with = "time::serde::rfc3339")]
    as_of: OffsetDateTime,
    sources_indexed: u64,
    live_objects: u64,
    countries: u64,
    languages: u64,
    new_objects_last_7_days: u64,
}

impl From<PublicCatalogueOverview> for PublicOverviewData {
    fn from(value: PublicCatalogueOverview) -> Self {
        Self {
            schema_version: PUBLIC_OVERVIEW_SCHEMA_VERSION,
            as_of: value.as_of,
            sources_indexed: value.sources_indexed,
            live_objects: value.live_objects,
            countries: value.countries,
            languages: value.languages,
            new_objects_last_7_days: value.new_objects_last_7_days,
        }
    }
}

/// The representation has no inputs: query strings and credentials are never evaluated, so
/// they cannot change it. A request carrying `Authorization` is still answered privately.
pub(crate) async fn get_public_overview(
    State(state): State<PublicOverviewState>,
    headers: HeaderMap,
) -> Response {
    let context = TransportPrincipal::Anonymous.operation_context(request_metadata(&headers));

    match state.get_overview.execute(&context).await {
        Ok(overview) => cache::anonymous_shared_success(
            Json(PublicOverviewData::from(overview)).into_response(),
            &headers,
            true,
            PUBLIC_OVERVIEW_SHARED_TTL_SECONDS,
        ),
        Err(error) => cache::private_no_store(ApiError::from(error).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{OperationContext, Principal},
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use product_listing_service::use_cases::{
        GetPublicCatalogueOverviewError, GetPublicCatalogueOverviewUseCase,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;
    use tower::ServiceExt;

    const PUBLIC_CACHE_CONTROL: &str = "public, max-age=0, s-maxage=21600, stale-if-error=0";

    type Requests = Arc<Mutex<Vec<OperationContext>>>;

    #[derive(Clone, Copy)]
    enum Outcome {
        Overview,
        Unavailable,
        InvalidReadModel,
    }

    struct FakeGetPublicCatalogueOverviewUseCase {
        requests: Requests,
        outcome: Outcome,
    }

    #[async_trait::async_trait]
    impl GetPublicCatalogueOverviewUseCase for FakeGetPublicCatalogueOverviewUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
        ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError> {
            lock(&self.requests).push(context.clone());
            match self.outcome {
                Outcome::Overview => Ok(overview()),
                Outcome::Unavailable => Err(
                    GetPublicCatalogueOverviewError::ReaderTemporarilyUnavailable {
                        source: static_error("reader unavailable"),
                    },
                ),
                Outcome::InvalidReadModel => {
                    Err(GetPublicCatalogueOverviewError::ReaderInvalidReadModel {
                        source: static_error("invalid read model"),
                    })
                }
            }
        }
    }

    fn overview() -> PublicCatalogueOverview {
        PublicCatalogueOverview {
            as_of: datetime!(2026-10-19 12:34:56.789 UTC),
            sources_indexed: 12,
            live_objects: 3456,
            countries: 7,
            languages: 5,
            new_objects_last_7_days: 89,
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn app(outcome: Outcome, requests: Requests) -> Router {
        crate::transport::with_transport_middleware(
            Router::new()
                .route("/api/v1/overview", axum::routing::get(get_public_overview))
                .with_state(PublicOverviewState::new(Arc::new(
                    FakeGetPublicCatalogueOverviewUseCase { requests, outcome },
                ))),
            crate::transport::NATIVE_REQUEST_TIMEOUT,
        )
    }

    async fn get(app: Router, uri: &str, authorization: Option<&str>) -> Response {
        let mut request = Request::get(uri);
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        app.oneshot(
            request
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("failed to build request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("request failed: {error}"))
    }

    fn cache_control(response: &Response) -> Option<&str> {
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    }

    async fn body(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response: {error}"))
    }

    #[tokio::test]
    async fn should_share_anonymous_overview_for_six_hours() {
        let requests = Arc::new(Mutex::new(Vec::new()));

        let response = get(
            app(Outcome::Overview, Arc::clone(&requests)),
            "/api/v1/overview",
            None,
        )
        .await;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(Some(PUBLIC_CACHE_CONTROL), cache_control(&response));
        assert_eq!(
            Some("Authorization, Origin"),
            response
                .headers()
                .get(header::VARY)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            serde_json::json!({
                "schemaVersion": 1,
                "asOf": "2026-10-19T12:34:56.789Z",
                "sourcesIndexed": 12,
                "liveObjects": 3456,
                "countries": 7,
                "languages": 5,
                "newObjectsLast7Days": 89
            }),
            body(response).await
        );
        let requests = lock(&requests);
        assert_eq!(1, requests.len());
        assert_eq!(Principal::Anonymous, requests[0].principal);
    }

    #[tokio::test]
    async fn should_ignore_query_strings_for_representation_and_cache_headers() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut representations = Vec::new();

        for uri in [
            "/api/v1/overview",
            "/api/v1/overview?",
            "/api/v1/overview?utm_source=landing",
            "/api/v1/overview?schemaVersion=2&language=de&currency=USD",
            "/api/v1/overview?asOf=invalid&asOf=duplicate",
        ] {
            let response = get(app(Outcome::Overview, Arc::clone(&requests)), uri, None).await;
            assert_eq!(StatusCode::OK, response.status(), "{uri}");
            assert_eq!(
                Some(PUBLIC_CACHE_CONTROL),
                cache_control(&response),
                "{uri}"
            );
            representations.push(body(response).await);
        }

        assert!(representations.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(5, lock(&requests).len());
    }

    #[tokio::test]
    async fn should_answer_authorization_bearing_requests_privately_without_evaluating_credentials()
    {
        for authorization in ["Bearer valid-looking-token", "malformed credentials"] {
            let requests = Arc::new(Mutex::new(Vec::new()));

            let response = get(
                app(Outcome::Overview, Arc::clone(&requests)),
                "/api/v1/overview",
                Some(authorization),
            )
            .await;

            assert_eq!(StatusCode::OK, response.status());
            assert_eq!(Some("private, no-store"), cache_control(&response));
            assert_eq!(Principal::Anonymous, lock(&requests)[0].principal);
        }
    }

    #[tokio::test]
    async fn should_not_cache_overview_errors() {
        for (outcome, status, error) in [
            (
                Outcome::Unavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "OVERVIEW_TEMPORARILY_UNAVAILABLE",
            ),
            (
                Outcome::InvalidReadModel,
                StatusCode::INTERNAL_SERVER_ERROR,
                "OVERVIEW_INTERNAL_ERROR",
            ),
        ] {
            let response = get(
                app(outcome, Arc::new(Mutex::new(Vec::new()))),
                "/api/v1/overview",
                None,
            )
            .await;

            assert_eq!(status, response.status());
            assert_eq!(Some("private, no-store"), cache_control(&response));
            assert_eq!(error, body(response).await["error"]);
        }
    }
}
