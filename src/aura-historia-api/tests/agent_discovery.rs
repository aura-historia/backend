//! Discovery documents link to documents that other hosts serve and describe operations of the
//! API and MCP server. Resolve every such link and name against the routers, documentation and
//! manifests in this repository, so a renamed path cannot leave a document pointing at nothing.
use agent_discovery::{DiscoveryEnvironment, SiteDocuments};
use aura_historia_api::{agent_discovery::AgentDiscoveryState, app, state::AppState};
use aura_historia_mcp::{AnonymousGate, Registry, TransportConfig, router};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use oauth_client_metadata_http::{FETCH_TIMEOUT, HttpClientMetadataDocumentSource};
use oauth_core::{
    client_identifier::OAuthClientIdentifier, metadata_document_client::ClientMetadataDocument,
    resource::AuraResourcePolicy,
};
use oauth_service::ports::ClientMetadataDocumentSource;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, net::SocketAddr, path::Path, sync::Arc};
use tower::ServiceExt;

const POLICIES: [AuraResourcePolicy; 2] = [
    AuraResourcePolicy::production(),
    AuraResourcePolicy::stage(),
];

/// The API and MCP routers of one environment, each answering for its own host.
struct Hosts {
    policy: AuraResourcePolicy,
    api: Router,
    mcp: Router,
}

impl Hosts {
    fn new(policy: AuraResourcePolicy) -> Self {
        let mcp_host = url::Url::parse(policy.mcp_origin()).unwrap();
        let mcp = router(
            TransportConfig {
                resource_policy: policy,
                allowed_hosts: vec![mcp_host.host_str().unwrap().to_owned()],
                ..Default::default()
            },
            Registry::default(),
            Arc::new(AnonymousGate),
        )
        .unwrap();
        Self {
            policy,
            api: app(AppState::new().with_agent_discovery(AgentDiscoveryState::new(policy))),
            mcp,
        }
    }

    /// Resolves a link to the documentation file or the in-process route that serves it.
    async fn assert_resolves(&self, url: &url::Url) {
        let origin = url.origin().ascii_serialization();
        if origin == self.policy.documentation_origin() {
            assert!(documentation_file(url).is_file(), "{url}");
            return;
        }
        let router = if origin == self.policy.rest_resource() {
            &self.api
        } else {
            assert_eq!(self.policy.mcp_origin(), origin, "{url}");
            &self.mcp
        };
        let status = get(router, url).await;
        assert!(status.is_success(), "{url} returned {status}");
    }
}

async fn get(router: &Router, url: &url::Url) -> StatusCode {
    let request = Request::builder()
        .uri(url.path())
        .header(header::HOST, url.host_str().unwrap())
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(request).await.unwrap().status()
}

fn documentation_file(url: &url::Url) -> std::path::PathBuf {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    match url.path() {
        "/" => docs.join("index.html"),
        path => docs.join(path.trim_start_matches('/')),
    }
}

/// Absolute URLs in a document's text, without surrounding Markdown or JSON punctuation.
fn urls(text: &str) -> Vec<&str> {
    text.match_indices("https://")
        .map(|(start, _)| {
            let rest = &text[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "`\"'()<>[]|,".contains(c))
                .unwrap_or(rest.len());
            rest[..end].trim_end_matches(['.', ':', ';'])
        })
        .collect()
}

#[tokio::test]
async fn should_resolve_every_catalog_link_served_from_this_repository() {
    for policy in POLICIES {
        let hosts = Hosts::new(policy);
        let api_host = url::Url::parse(policy.rest_resource()).unwrap();
        let catalog = hosts
            .api
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/api-catalog")
                    .header(header::HOST, api_host.host_str().unwrap())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let catalog: Value =
            serde_json::from_slice(&to_bytes(catalog.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        let mut resolved = 0;
        for context in catalog["linkset"].as_array().unwrap() {
            for relation in ["service-desc", "service-doc", "status"] {
                for link in context[relation].as_array().into_iter().flatten() {
                    let url = url::Url::parse(link["href"].as_str().unwrap()).unwrap();
                    hosts.assert_resolves(&url).await;
                    resolved += 1;
                }
            }
        }
        assert_eq!(6, resolved);
    }
}

/// Website links resolve in the agent-discovery crate's own tests. Endpoints such as the MCP
/// `/v1`, the REST `/v1` base and the `POST`-only revocation endpoint are named, not fetched.
/// The authorization-server metadata needs the OAuth state, and its route is covered by the
/// OAuth discovery tests.
#[tokio::test]
async fn should_resolve_every_site_document_link_to_another_host() {
    for policy in POLICIES {
        let hosts = Hosts::new(policy);
        let environment = DiscoveryEnvironment::from(policy);
        let named_only = [
            environment.api_origin().to_owned(),
            format!("{}/v1", environment.api_origin()),
            environment.mcp_endpoint().to_owned(),
            format!(
                "{}/.well-known/oauth-authorization-server",
                environment.issuer()
            ),
            format!("{}/v1/oauth/revoke", environment.issuer()),
        ];
        let mut resolved = 0;
        for document in SiteDocuments::render(environment).iter() {
            for url in urls(document.body()) {
                if url.starts_with(environment.site_origin())
                    || url.starts_with("https://schemas.")
                    || named_only.iter().any(|named| named == url)
                {
                    continue;
                }
                hosts.assert_resolves(&url::Url::parse(url).unwrap()).await;
                resolved += 1;
            }
        }
        assert!(resolved >= 8, "{resolved}");
    }
}

/// Fetches the skills index from the API and every skill it lists, at the API path of the
/// website URL the index names, and hashes the bytes the router returns.
#[tokio::test]
async fn should_list_each_skill_with_the_digest_of_the_bytes_the_api_serves() {
    for policy in POLICIES {
        let hosts = Hosts::new(policy);
        let environment = DiscoveryEnvironment::from(policy);
        let documents = SiteDocuments::render(environment);
        let index: Value = serde_json::from_slice(
            &served(&hosts.api, "/v1/site-discovery/agent-skills/index.json").await,
        )
        .unwrap();
        let skills = index["skills"].as_array().unwrap();
        assert_eq!(3, skills.len());
        for skill in skills {
            let url = skill["url"].as_str().unwrap();
            let document = documents
                .iter()
                .find(|document| {
                    url == format!("{}{}", environment.site_origin(), document.site_path())
                })
                .expect("every listed skill is a website document");
            let body = served(
                &hosts.api,
                &format!("/v1/site-discovery/{}", document.path()),
            )
            .await;
            let hex: String = Sha256::digest(&body)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(format!("sha256:{hex}"), skill["digest"], "{url}");
        }
    }
}

/// The skills and auth.md name REST routes and MCP tools. Every backticked `/v1/…` route, with
/// its method where one is given, must be an OpenAPI operation, and every tool in an
/// "MCP tool" table column must be in an MCP operation manifest.
#[test]
fn should_name_only_operations_that_the_api_and_mcp_server_offer() {
    let operations = swagger_operations();
    let tools = mcp_tools();
    let documents = SiteDocuments::render(AuraResourcePolicy::production().into());
    let (mut routes, mut named_tools) = (0, 0);
    for document in documents
        .iter()
        .filter(|document| document.path().ends_with(".md"))
    {
        for span in backticked(document.body()) {
            let (method, path) = match span.split_once(' ') {
                Some((method, path)) if method.bytes().all(|b| b.is_ascii_uppercase()) => {
                    (Some(method.to_ascii_lowercase()), path)
                }
                _ => (None, span),
            };
            if !path.starts_with("/v1/") || path.contains(' ') {
                continue;
            }
            let path = normalize(path);
            assert!(
                operations
                    .iter()
                    .any(|(m, p)| *p == path && method.as_ref().is_none_or(|method| m == method)),
                "`{span}` in {} is not an OpenAPI operation",
                document.path()
            );
            routes += 1;
        }
        for tool in mcp_tool_column(document.body()) {
            assert!(
                tools.contains(tool),
                "{tool} in {} is no MCP tool",
                document.path()
            );
            named_tools += 1;
        }
    }
    assert!(routes >= 15, "{routes}");
    assert_eq!(11, named_tools);
}

/// Stands in for the website's Worker: serves each website path from the API path below
/// `/v1/site-discovery`, unchanged, on a loopback origin.
async fn website(api: Router) -> SocketAddr {
    let worker = Router::new().fallback(move |request: Request<Body>| {
        let api = api.clone();
        async move {
            let path = format!("/v1/site-discovery{}", request.uri().path());
            api.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, worker).await.unwrap() });
    address
}

/// The authorization and token endpoints load the website's WebMCP client like any other
/// metadata document client: the document source fetches it from the website under its fetch
/// limits, and the shared validator accepts it for the `client_id` it was fetched from.
#[tokio::test]
async fn should_load_the_webmcp_client_from_the_website_as_the_oauth_endpoints_do() {
    for policy in POLICIES {
        let environment = DiscoveryEnvironment::from(policy);
        let source = HttpClientMetadataDocumentSource::with_loopback_test_origin(
            environment.site_domain(),
            website(Hosts::new(policy).api).await,
            FETCH_TIMEOUT,
        );
        let url = format!("{}/oauth/webmcp-client.json", environment.site_origin());
        let OAuthClientIdentifier::MetadataDocument(client_id) =
            OAuthClientIdentifier::parse(&url).unwrap()
        else {
            panic!("{url} is not a metadata document client ID");
        };

        let claims = source.load(&client_id).await.unwrap();
        let document = ClientMetadataDocument::validate(&client_id, claims)
            .unwrap_or_else(|error| panic!("{url}: {error}"));

        assert!(document.declares_refresh_token_grant());
        let callback =
            |origin: &str| url::Url::parse(&format!("{origin}/api/oauth/webmcp/callback")).unwrap();
        assert!(document.admits_redirect_uri(&callback(environment.site_origin())));
        assert_eq!(
            policy == AuraResourcePolicy::stage(),
            document.admits_redirect_uri(&callback("http://localhost:3000")),
            "only stage admits the local development callback"
        );
    }
}

async fn served(router: &Router, path: &str) -> Vec<u8> {
    let response = router
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status(), "{path}");
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

fn backticked(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').skip(1).step_by(2)
}

/// Path parameters compare by position, not by name.
fn normalize(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "{}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `(method, normalized path)` of every OpenAPI operation.
fn swagger_operations() -> Vec<(String, String)> {
    let swagger = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/swagger.yaml"),
    )
    .unwrap();
    let mut path = None;
    let mut operations = Vec::new();
    for line in swagger.lines() {
        if let Some(route) = line
            .strip_prefix("  /")
            .and_then(|rest| rest.strip_suffix(':'))
        {
            path = Some(normalize(&format!("/{route}")));
        } else if let Some(method) = line
            .strip_prefix("    ")
            .and_then(|rest| rest.strip_suffix(':'))
            .filter(|method| ["get", "post", "put", "patch", "delete", "head"].contains(method))
            && let Some(path) = &path
        {
            operations.push((method.to_owned(), path.clone()));
        }
    }
    operations
}

fn mcp_tools() -> BTreeSet<String> {
    ["public-operations.json", "authenticated-operations.json"]
        .into_iter()
        .flat_map(|manifest| {
            let manifest: Value = serde_json::from_str(
                &std::fs::read_to_string(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../aura-historia-mcp")
                        .join(manifest),
                )
                .unwrap(),
            )
            .unwrap();
            manifest["operations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|operation| operation["operationId"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Backticked names in the "MCP tool" column of Markdown tables.
fn mcp_tool_column(markdown: &str) -> Vec<&str> {
    let mut column = None;
    let mut tools = Vec::new();
    for line in markdown.lines() {
        let Some(row) = line.strip_prefix('|') else {
            column = None;
            continue;
        };
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        match column {
            None => column = cells.iter().position(|cell| *cell == "MCP tool"),
            Some(index) => tools.extend(backticked(cells[index])),
        }
    }
    tools
}
