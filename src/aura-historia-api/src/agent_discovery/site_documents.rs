use super::AgentDiscoveryState;
use crate::transport::cache;
use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

/// The API serves the website's discovery documents below this path; the webapp publishes them
/// at the website's own `/.well-known` paths.
pub(crate) const SITE_DISCOVERY_PREFIX: &str = "/v1/site-discovery/";
const SITE_DOCUMENT_SHARED_TTL_SECONDS: u32 = 300;

/// `GET` returns one website discovery document and `HEAD` its headers. The routes are exact;
/// a skill name the environment does not publish is not found. Credentials are never evaluated,
/// and a request carrying `Authorization` is answered privately.
pub(crate) async fn get_site_document(
    State(state): State<AgentDiscoveryState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let Some(document) = uri
        .path()
        .strip_prefix(SITE_DISCOVERY_PREFIX)
        .and_then(|path| state.site_documents.get(path))
    else {
        return cache::private_no_store(StatusCode::NOT_FOUND.into_response());
    };
    let response = (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(document.media_type()),
        )],
        document.body().to_owned(),
    )
        .into_response();
    cache::anonymous_shared_success(response, &headers, true, SITE_DOCUMENT_SHARED_TTL_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_discovery::DiscoveryEnvironment;
    use oauth_core::resource::AuraResourcePolicy;

    async fn get(path: &str, headers: HeaderMap) -> (Response, String) {
        let response = get_site_document(
            State(AgentDiscoveryState::new(AuraResourcePolicy::stage())),
            path.parse().unwrap(),
            headers,
        )
        .await;
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (
            Response::from_parts(parts, axum::body::Body::empty()),
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    #[tokio::test]
    async fn should_serve_every_document_with_its_media_type_and_shared_caching() {
        let documents = agent_discovery::SiteDocuments::render(DiscoveryEnvironment::from(
            AuraResourcePolicy::stage(),
        ));
        for document in documents.iter() {
            let (response, body) = get(
                &format!("{SITE_DISCOVERY_PREFIX}{}", document.path()),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(StatusCode::OK, response.status(), "{}", document.path());
            assert_eq!(
                document.media_type(),
                response.headers()[header::CONTENT_TYPE]
            );
            assert_eq!(
                "public, max-age=0, s-maxage=300, stale-if-error=0",
                response.headers()[header::CACHE_CONTROL]
            );
            assert_eq!(document.body(), body);
        }
    }

    #[tokio::test]
    async fn should_not_find_a_skill_the_environment_does_not_publish() {
        let (response, _) = get(
            "/v1/site-discovery/agent-skills/unknown/SKILL.md",
            HeaderMap::new(),
        )
        .await;
        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
    }

    #[tokio::test]
    async fn should_answer_privately_when_a_credential_is_supplied() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        let (response, _) = get("/v1/site-discovery/auth.md", headers).await;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
    }
}
