//! Discovery documents link to documents that other hosts serve. Resolve every such link against
//! the routers and documentation in this repository, so a renamed path cannot leave a document
//! pointing at nothing.
use agent_discovery::{DiscoveryEnvironment, SiteDocuments};
use aura_historia_api::{agent_discovery::AgentDiscoveryState, app, state::AppState};
use aura_historia_mcp::{AnonymousGate, Registry, TransportConfig, router};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use oauth_core::resource::AuraResourcePolicy;
use serde_json::Value;
use std::{path::Path, sync::Arc};
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

/// Website links resolve in the agent-discovery crate's own tests; endpoints such as the MCP
/// `/v1` and the REST `/v1` base are named, not fetched. The authorization-server metadata
/// needs the OAuth state, and its route is covered by the OAuth discovery tests.
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
