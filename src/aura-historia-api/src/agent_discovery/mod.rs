//! Machine-readable documents that describe the API host to agents. The
//! [agent discovery contract](../../../../docs/agent-discovery.md) owns their shapes.
pub(crate) mod api_catalog;
pub(crate) mod protected_resource;

use oauth_core::resource::AuraResourcePolicy;

/// The environment that `OAUTH_ISSUER_URL` selects; every advertised URL comes from it.
#[derive(Clone)]
pub struct AgentDiscoveryState {
    pub(crate) resource_policy: AuraResourcePolicy,
}

impl AgentDiscoveryState {
    pub fn new(resource_policy: AuraResourcePolicy) -> Self {
        Self { resource_policy }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app,
        auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal},
        state::{AppState, NewsletterState},
    };
    use application::operation_context::OperationContext;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_service::use_cases::{
        ConfirmNewsletterSubscriptionError, ConfirmNewsletterSubscriptionUseCase,
        RequestNewsletterSubscriptionCommand, RequestNewsletterSubscriptionError,
        RequestNewsletterSubscriptionUseCase,
    };

    struct RejectingAuthenticator;
    #[async_trait::async_trait]
    impl TokenAuthenticator for RejectingAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Err(AuthError::InvalidCredentials)
        }
    }

    struct UnusedNewsletter;
    #[async_trait::async_trait]
    impl RequestNewsletterSubscriptionUseCase for UnusedNewsletter {
        async fn execute(
            &self,
            _: &OperationContext,
            _: RequestNewsletterSubscriptionCommand,
        ) -> Result<(), RequestNewsletterSubscriptionError> {
            unreachable!("authentication fails first")
        }
    }
    #[async_trait::async_trait]
    impl ConfirmNewsletterSubscriptionUseCase for UnusedNewsletter {
        async fn execute(&self, _: &str) -> Result<(), ConfirmNewsletterSubscriptionError> {
            unreachable!("not requested")
        }
    }

    fn state() -> AppState {
        AppState::new().with_newsletter(NewsletterState::new(
            Arc::new(UnusedNewsletter),
            Arc::new(UnusedNewsletter),
            Arc::new(RejectingAuthenticator),
        ))
    }

    fn rejected_bearer() -> Request<Body> {
        Request::builder()
            .method(Method::PUT)
            .uri("/v1/newsletter-subscriptions")
            .header(header::AUTHORIZATION, "Bearer rejected")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap()
    }

    fn get(method: Method, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::ORIGIN, "https://any-agent.example")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn should_point_rejected_bearers_to_the_rest_protected_resource_metadata() {
        let router = app(
            state().with_agent_discovery(AgentDiscoveryState::new(AuraResourcePolicy::stage()))
        );
        let response = router.oneshot(rejected_bearer()).await.unwrap();
        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            "Bearer resource_metadata=\"https://api.stage.aura-historia.com/.well-known/oauth-protected-resource\"",
            response.headers()[header::WWW_AUTHENTICATE]
        );

        let response = app(state()).oneshot(rejected_bearer()).await.unwrap();
        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    #[tokio::test]
    async fn should_serve_discovery_documents_through_the_transport_policy() {
        let router = app(AppState::new()
            .with_agent_discovery(AgentDiscoveryState::new(AuraResourcePolicy::production())));
        let metadata = router
            .clone()
            .oneshot(get(Method::GET, "/.well-known/oauth-protected-resource"))
            .await
            .unwrap();
        assert_eq!(StatusCode::OK, metadata.status());
        assert_eq!("no-store", metadata.headers()[header::CACHE_CONTROL]);
        assert_eq!("*", metadata.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN]);

        for method in [Method::GET, Method::HEAD] {
            let catalog = router
                .clone()
                .oneshot(get(method.clone(), "/.well-known/api-catalog"))
                .await
                .unwrap();
            assert_eq!(StatusCode::OK, catalog.status(), "{method}");
            assert_eq!(
                "public, max-age=0, s-maxage=300, stale-if-error=0",
                catalog.headers()[header::CACHE_CONTROL]
            );
            assert_eq!("*", catalog.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN]);
            assert_eq!(
                "<https://api.aura-historia.com/.well-known/api-catalog>; rel=\"api-catalog\"",
                catalog.headers()[header::LINK]
            );
        }
    }
}
