//! The API catalog links to documents that other hosts serve. Resolve every such link against
//! the routers and documentation in this repository, so a renamed path cannot leave the
//! catalog pointing at nothing.
use aura_historia_api::{agent_discovery::AgentDiscoveryState, app, state::AppState};
use aura_historia_mcp::{AnonymousGate, Registry, TransportConfig, router};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, header},
};
use oauth_core::resource::AuraResourcePolicy;
use serde_json::Value;
use std::{path::Path, sync::Arc};
use tower::ServiceExt;

async fn get(router: &Router, url: &url::Url) -> axum::http::StatusCode {
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

#[tokio::test]
async fn should_resolve_every_catalog_link_served_from_this_repository() {
    for policy in [
        AuraResourcePolicy::production(),
        AuraResourcePolicy::stage(),
    ] {
        let api = app(AppState::new().with_agent_discovery(AgentDiscoveryState::new(policy)));
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
        let api_host = url::Url::parse(policy.rest_resource()).unwrap();
        let catalog = api
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
                    let origin = url.origin().ascii_serialization();
                    if origin == policy.documentation_origin() {
                        assert!(documentation_file(&url).is_file(), "{url}");
                    } else {
                        let status = if origin == policy.rest_resource() {
                            get(&api, &url).await
                        } else {
                            assert_eq!(policy.mcp_origin(), origin);
                            get(&mcp, &url).await
                        };
                        assert!(status.is_success(), "{url} returned {status}");
                    }
                    resolved += 1;
                }
            }
        }
        assert_eq!(6, resolved);
    }
}
