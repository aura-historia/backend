use super::AgentDiscoveryState;
use crate::transport::cache;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use oauth_core::resource::AuraResourcePolicy;
use serde::Serialize;

pub(crate) const API_CATALOG_PATH: &str = "/.well-known/api-catalog";
/// [RFC 9727](https://www.rfc-editor.org/rfc/rfc9727) catalogs use the Linkset format with
/// the catalog profile.
const API_CATALOG_MEDIA_TYPE: &str =
    "application/linkset+json; profile=\"https://www.rfc-editor.org/info/rfc9727\"";
/// The MCP host serves the card at this path; the MCP crate owns that route.
const MCP_SERVER_CARD_PATH: &str = "/.well-known/mcp/server-card.json";
const API_CATALOG_SHARED_TTL_SECONDS: u32 = 300;

#[derive(Serialize)]
struct ApiCatalog {
    linkset: [LinkContext; 3],
}

/// One Linkset context object ([RFC 9264](https://www.rfc-editor.org/rfc/rfc9264)).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct LinkContext {
    anchor: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    item: Vec<Link>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    service_desc: Vec<Link>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    service_doc: Vec<Link>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    status: Vec<Link>,
}

#[derive(Serialize)]
struct Link {
    href: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    media_type: Option<&'static str>,
}

impl Link {
    fn new(href: String) -> Self {
        Self {
            href,
            media_type: None,
        }
    }

    fn typed(href: String, media_type: &'static str) -> Self {
        Self {
            href,
            media_type: Some(media_type),
        }
    }
}

impl ApiCatalog {
    /// The catalog's own context lists both APIs as items; each API context links its
    /// description, documentation and status probe.
    fn new(policy: AuraResourcePolicy) -> Self {
        let api = policy.rest_resource();
        let mcp = policy.mcp_origin();
        let docs = policy.documentation_origin();
        let rest_endpoint = format!("{api}/v1");
        Self {
            linkset: [
                LinkContext {
                    anchor: catalog_url(policy),
                    item: vec![
                        Link::new(rest_endpoint.clone()),
                        Link::new(policy.mcp_resource().to_owned()),
                    ],
                    ..LinkContext::empty()
                },
                LinkContext {
                    service_desc: vec![Link::typed(
                        format!("{docs}/swagger.yaml"),
                        "application/vnd.oai.openapi",
                    )],
                    service_doc: vec![Link::typed(format!("{docs}/"), "text/html")],
                    status: vec![Link::new(format!("{rest_endpoint}/health"))],
                    ..LinkContext::anchored(rest_endpoint)
                },
                LinkContext {
                    service_desc: vec![Link::typed(
                        format!("{mcp}{MCP_SERVER_CARD_PATH}"),
                        "application/mcp-server-card+json",
                    )],
                    service_doc: vec![Link::typed(format!("{docs}/mcp.md"), "text/markdown")],
                    status: vec![Link::new(format!("{mcp}/ready"))],
                    ..LinkContext::anchored(policy.mcp_resource().to_owned())
                },
            ],
        }
    }
}

impl LinkContext {
    fn anchored(anchor: String) -> Self {
        Self {
            anchor,
            ..Self::empty()
        }
    }

    fn empty() -> Self {
        Self {
            anchor: String::new(),
            item: Vec::new(),
            service_desc: Vec::new(),
            service_doc: Vec::new(),
            status: Vec::new(),
        }
    }
}

fn catalog_url(policy: AuraResourcePolicy) -> String {
    format!("{}{API_CATALOG_PATH}", policy.rest_resource())
}

/// `GET` returns the catalog and `HEAD` its headers; both carry the `api-catalog` link that
/// RFC 9727 requires on `HEAD`. Credentials are never evaluated, and a request carrying
/// `Authorization` is answered privately.
pub(crate) async fn get_api_catalog(
    State(state): State<AgentDiscoveryState>,
    headers: HeaderMap,
) -> Response {
    let policy = state.resource_policy;
    let mut response = Json(ApiCatalog::new(policy)).into_response();
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(API_CATALOG_MEDIA_TYPE),
    );
    response_headers.insert(
        header::LINK,
        HeaderValue::try_from(format!("<{}>; rel=\"api-catalog\"", catalog_url(policy)))
            .expect("closed policy origins are valid header values"),
    );
    cache::anonymous_shared_success(response, &headers, true, API_CATALOG_SHARED_TTL_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    async fn catalog(policy: AuraResourcePolicy, headers: HeaderMap) -> (Response, Value) {
        let response = get_api_catalog(State(AgentDiscoveryState::new(policy)), headers).await;
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (
            Response::from_parts(parts, axum::body::Body::empty()),
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    fn hrefs(value: &Value) -> Vec<&str> {
        match value {
            Value::Object(members) => members
                .iter()
                .flat_map(|(name, value)| match (name.as_str(), value) {
                    ("href" | "anchor", Value::String(url)) => vec![url.as_str()],
                    _ => hrefs(value),
                })
                .collect(),
            Value::Array(values) => values.iter().flat_map(hrefs).collect(),
            _ => vec![],
        }
    }

    #[tokio::test]
    async fn should_catalog_rest_and_mcp_with_their_descriptions_docs_and_status() {
        let (response, body) = catalog(AuraResourcePolicy::production(), HeaderMap::new()).await;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "application/linkset+json; profile=\"https://www.rfc-editor.org/info/rfc9727\"",
            response.headers()[header::CONTENT_TYPE]
        );
        assert_eq!(
            "<https://api.aura-historia.com/.well-known/api-catalog>; rel=\"api-catalog\"",
            response.headers()[header::LINK]
        );
        assert_eq!(
            "public, max-age=0, s-maxage=300, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!(
            json!({"linkset": [
                {
                    "anchor": "https://api.aura-historia.com/.well-known/api-catalog",
                    "item": [{"href": "https://api.aura-historia.com/v1"}, {"href": "https://mcp.aura-historia.com/v1"}],
                },
                {
                    "anchor": "https://api.aura-historia.com/v1",
                    "service-desc": [{"href": "https://docs.api.aura-historia.com/swagger.yaml", "type": "application/vnd.oai.openapi"}],
                    "service-doc": [{"href": "https://docs.api.aura-historia.com/", "type": "text/html"}],
                    "status": [{"href": "https://api.aura-historia.com/v1/health"}],
                },
                {
                    "anchor": "https://mcp.aura-historia.com/v1",
                    "service-desc": [{"href": "https://mcp.aura-historia.com/.well-known/mcp/server-card.json", "type": "application/mcp-server-card+json"}],
                    "service-doc": [{"href": "https://docs.api.aura-historia.com/mcp.md", "type": "text/markdown"}],
                    "status": [{"href": "https://mcp.aura-historia.com/ready"}],
                },
            ]}),
            body
        );
    }

    #[tokio::test]
    async fn should_link_only_to_the_configured_environment() {
        for policy in [
            AuraResourcePolicy::production(),
            AuraResourcePolicy::stage(),
        ] {
            let (response, body) = catalog(policy, HeaderMap::new()).await;
            let origins = [
                policy.rest_resource(),
                policy.mcp_origin(),
                policy.documentation_origin(),
            ];
            let urls = hrefs(&body);
            assert_eq!(11, urls.len());
            for url in urls {
                assert!(
                    origins
                        .iter()
                        .any(|origin| url.starts_with(&format!("{origin}/"))),
                    "{url} is outside the configured environment"
                );
            }
            assert!(
                response.headers()[header::LINK]
                    .to_str()
                    .unwrap()
                    .starts_with(&format!("<{}/", policy.rest_resource()))
            );
        }
    }

    #[tokio::test]
    async fn should_answer_privately_when_a_credential_is_supplied() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        let (response, _) = catalog(AuraResourcePolicy::stage(), headers).await;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
    }
}
