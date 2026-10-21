use crate::{AccessClass, AdmissionGate, AdmissionRejection, AdmissionRequest, OperationPolicy};
use application::{error::box_error, operation_context::Principal};
use async_trait::async_trait;
use aura_historia_auth::{AccessTokenAuthenticationError, authenticate_access_token};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use oauth_core::resource::AuraResourcePolicy;
use std::{collections::BTreeSet, sync::Arc};
use user_service::use_cases::AuthenticateAccessTokenUseCase;

/// Validates every supplied Aura credential against the current authoritative token store.
/// This object caches use-case handles, never credentials, principals or session authority.
pub struct AuraTokenGate {
    authentication: Arc<dyn AuthenticateAccessTokenUseCase>,
    resource: AuraResourcePolicy,
}

impl AuraTokenGate {
    pub fn new(
        authentication: Arc<dyn AuthenticateAccessTokenUseCase>,
        resource: AuraResourcePolicy,
    ) -> Self {
        Self {
            authentication,
            resource,
        }
    }

    fn challenge(
        &self,
        status: StatusCode,
        error: Option<&str>,
        policy: Option<&OperationPolicy>,
    ) -> AdmissionRejection {
        let mut value = format!(
            "Bearer resource_metadata=\"{}\"",
            resource_metadata_url(self.resource)
        );
        if let Some(error) = error {
            value.push_str(&format!(", error=\"{error}\""));
        }
        if let Some(policy) = policy {
            let scopes = policy
                .required_scopes
                .iter()
                .map(|scope| scope.as_scope_str())
                .collect::<BTreeSet<_>>();
            if !scopes.is_empty() {
                value.push_str(&format!(
                    ", scope=\"{}\"",
                    scopes.into_iter().collect::<Vec<_>>().join(" ")
                ));
            }
        }
        let mut rejection = AdmissionRejection::new(status);
        match HeaderValue::from_str(&value) {
            Ok(value) => {
                rejection.headers.insert(header::WWW_AUTHENTICATE, value);
            }
            Err(source) => {
                return AdmissionRejection::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_source(box_error(source));
            }
        }
        rejection
    }

    fn authentication_failure(
        &self,
        error: AccessTokenAuthenticationError,
        policy: Option<&OperationPolicy>,
    ) -> AdmissionRejection {
        match error {
            AccessTokenAuthenticationError::MalformedCredentials { .. }
            | AccessTokenAuthenticationError::InvalidCredentials => {
                self.challenge(StatusCode::UNAUTHORIZED, Some("invalid_token"), policy)
            }
            AccessTokenAuthenticationError::TemporarilyUnavailable { .. } => {
                let mut rejection = AdmissionRejection::new(StatusCode::SERVICE_UNAVAILABLE)
                    .with_source(box_error(error));
                rejection
                    .headers
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                rejection
            }
            AccessTokenAuthenticationError::Internal { .. } => {
                AdmissionRejection::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_source(box_error(error))
            }
        }
    }
}

#[async_trait]
impl AdmissionGate for AuraTokenGate {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        let policy = request
            .operation
            .map(|operation| operation.policy())
            .filter(|policy| policy.access == AccessClass::Protected);
        let token = bearer_token(request.headers).map_err(|()| {
            self.challenge(StatusCode::UNAUTHORIZED, Some("invalid_token"), policy)
        })?;
        let Some(token) = token else {
            return match policy {
                Some(_) => Err(self.challenge(StatusCode::UNAUTHORIZED, None, policy)),
                None => Ok(Principal::Anonymous),
            };
        };
        let identity =
            authenticate_access_token(self.authentication.as_ref(), token, request.context)
                .await
                .map_err(|error| self.authentication_failure(error, policy))?;
        if policy.is_some_and(|policy| {
            policy
                .required_scopes
                .iter()
                .any(|scope| !identity.capabilities.contains(scope))
        }) {
            return Err(self.challenge(StatusCode::FORBIDDEN, Some("insufficient_scope"), policy));
        }
        Ok(Principal::DelegatedUser {
            user_id: identity.user_id,
            capabilities: identity.capabilities,
        })
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<Option<&str>, ()> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    let value = value.to_str().map_err(|_| ())?;
    let (scheme, token) = value.split_once(' ').ok_or(())?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b',')
    {
        return Err(());
    }
    Ok(Some(token))
}

pub(crate) fn resource_metadata_url(policy: AuraResourcePolicy) -> String {
    // The shared policy supplies only canonical HTTPS resource URLs.
    let mut url = url::Url::parse(policy.mcp_resource()).expect("canonical Aura MCP resource URL");
    url.set_path("/.well-known/oauth-protected-resource/v1");
    url.into()
}
