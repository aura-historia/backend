use aws_lambda_events::cognito::{
    CognitoEventUserPoolsPreSignup, CognitoEventUserPoolsPreSignupTriggerSource,
};
use lambda_runtime::LambdaEvent;
use serde_email::Email;
use std::collections::HashSet;
use user_cognito::{CognitoAccountLinkError, CognitoAccountLinkOutcome, CognitoAccountLinker};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CognitoIdentityProviderLinkingPolicy {
    pub provider_name: String,
    pub auto_link_verified_email: bool,
    pub link_source_attribute_name: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PreSignUpError {
    #[error("invalid Cognito identity-provider linking policy")]
    InvalidPolicy,
    #[error("invalid external-provider sign-up event")]
    InvalidEvent,
    #[error("Cognito account linking is temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("invalid Cognito account-link state")]
    InvalidIdentityState,
}

impl PreSignUpError {
    pub const fn category(&self) -> &'static str {
        match self {
            Self::InvalidPolicy => "invalid_policy",
            Self::InvalidEvent => "invalid_event",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::InvalidIdentityState => "invalid_identity_state",
        }
    }
}

#[async_trait::async_trait]
pub trait FederatedIdentityLinker: Send + Sync {
    async fn link_account(
        &self,
        user_pool_id: &str,
        source_provider_name: &str,
        source_provider_attribute_name: &str,
        source_subject: &str,
        verified_email: &str,
    ) -> Result<CognitoAccountLinkOutcome, CognitoAccountLinkError>;
}

#[async_trait::async_trait]
impl FederatedIdentityLinker for CognitoAccountLinker {
    async fn link_account(
        &self,
        user_pool_id: &str,
        source_provider_name: &str,
        source_provider_attribute_name: &str,
        source_subject: &str,
        verified_email: &str,
    ) -> Result<CognitoAccountLinkOutcome, CognitoAccountLinkError> {
        CognitoAccountLinker::link_account(
            self,
            user_pool_id,
            source_provider_name,
            source_provider_attribute_name,
            source_subject,
            verified_email,
        )
        .await
    }
}

pub fn parse_linking_policy(
    value: &str,
) -> Result<Vec<CognitoIdentityProviderLinkingPolicy>, PreSignUpError> {
    let value: serde_json::Value =
        serde_json::from_str(value).map_err(|_| PreSignUpError::InvalidPolicy)?;
    let entries = value.as_array().ok_or(PreSignUpError::InvalidPolicy)?;
    let mut names = HashSet::with_capacity(entries.len());
    let mut policies = Vec::with_capacity(entries.len());

    for entry in entries {
        let provider_name = entry
            .get("providerName")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty() && name.trim() == *name)
            .ok_or(PreSignUpError::InvalidPolicy)?;
        let auto_link_verified_email = entry
            .get("autoLinkVerifiedEmail")
            .and_then(serde_json::Value::as_bool)
            .ok_or(PreSignUpError::InvalidPolicy)?;
        let link_source_attribute_name = entry
            .get("linkSourceAttributeName")
            .and_then(serde_json::Value::as_str)
            .filter(|attribute| !attribute.trim().is_empty() && attribute.trim() == *attribute)
            .ok_or(PreSignUpError::InvalidPolicy)?;
        if !names.insert(provider_name.to_owned()) {
            return Err(PreSignUpError::InvalidPolicy);
        }
        policies.push(CognitoIdentityProviderLinkingPolicy {
            provider_name: provider_name.to_owned(),
            auto_link_verified_email,
            link_source_attribute_name: link_source_attribute_name.to_owned(),
        });
    }
    Ok(policies)
}

#[tracing::instrument(skip(event, providers, linker), fields(request_id = %event.context.request_id))]
pub async fn handler<L: FederatedIdentityLinker>(
    event: LambdaEvent<CognitoEventUserPoolsPreSignup>,
    providers: &[CognitoIdentityProviderLinkingPolicy],
    linker: &L,
) -> Result<CognitoEventUserPoolsPreSignup, PreSignUpError> {
    let request_id = event.context.request_id.clone();
    let payload = event.payload;
    let trigger_source = payload
        .cognito_event_user_pools_header
        .trigger_source
        .as_ref();

    if !matches!(
        trigger_source,
        Some(CognitoEventUserPoolsPreSignupTriggerSource::ExternalProvider)
    ) {
        tracing::info!(
            request_id,
            trigger_kind = "non_external",
            result_category = "no_op",
        );
        return Ok(payload);
    }

    let username = payload
        .cognito_event_user_pools_header
        .user_name
        .as_deref()
        .ok_or(PreSignUpError::InvalidEvent)?;
    let (provider, source_subject) = resolve_provider(username, providers)?;

    if !provider.auto_link_verified_email {
        tracing::info!(
            request_id,
            trigger_kind = "external_provider",
            provider_name = %provider.provider_name,
            result_category = "linking_not_enabled",
        );
        return Ok(payload);
    }

    let email_value = payload
        .request
        .user_attributes
        .get("email")
        .ok_or(PreSignUpError::InvalidEvent)?;
    let email: Email = email_value
        .as_str()
        .try_into()
        .map_err(|_| PreSignUpError::InvalidEvent)?;
    let email = email.to_string();

    if payload
        .request
        .user_attributes
        .get("email_verified")
        .is_none_or(|verified| verified != "true")
    {
        return Err(PreSignUpError::InvalidEvent);
    }
    let user_pool_id = payload
        .cognito_event_user_pools_header
        .user_pool_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or(PreSignUpError::InvalidEvent)?;

    let outcome = linker
        .link_account(
            user_pool_id,
            &provider.provider_name,
            &provider.link_source_attribute_name,
            source_subject,
            &email,
        )
        .await
        .map_err(|error| match error {
            CognitoAccountLinkError::TemporarilyUnavailable => {
                PreSignUpError::TemporarilyUnavailable
            }
            CognitoAccountLinkError::InvalidState => PreSignUpError::InvalidIdentityState,
        })?;

    let result_category = match outcome {
        CognitoAccountLinkOutcome::NoExistingAccount => "no_existing_account",
        CognitoAccountLinkOutcome::Linked => "linked",
    };
    tracing::info!(
        request_id,
        trigger_kind = "external_provider",
        provider_name = %provider.provider_name,
        result_category,
    );
    Ok(payload)
}

fn resolve_provider<'a>(
    username: &'a str,
    providers: &'a [CognitoIdentityProviderLinkingPolicy],
) -> Result<(&'a CognitoIdentityProviderLinkingPolicy, &'a str), PreSignUpError> {
    let matches: Vec<_> = providers
        .iter()
        .filter_map(|provider| {
            username
                .strip_prefix(&provider.provider_name)
                .and_then(|suffix| suffix.strip_prefix('_'))
                .map(|subject| (provider, subject))
        })
        .collect();

    let Some(longest_length) = matches
        .iter()
        .map(|(provider, _)| provider.provider_name.len())
        .max()
    else {
        return Err(PreSignUpError::InvalidEvent);
    };
    let mut longest_matches = matches
        .into_iter()
        .filter(|(provider, _)| provider.provider_name.len() == longest_length);
    let matched = longest_matches.next().ok_or(PreSignUpError::InvalidEvent)?;
    if longest_matches.next().is_some() || matched.1.is_empty() {
        return Err(PreSignUpError::InvalidEvent);
    }
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::{Context, LambdaEvent};
    use std::sync::Mutex;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct LinkRequest {
        user_pool_id: String,
        source_provider_name: String,
        source_provider_attribute_name: String,
        source_subject: String,
        verified_email: String,
    }

    struct FakeLinker {
        calls: Mutex<Vec<LinkRequest>>,
        outcome: CognitoAccountLinkOutcome,
        failure: Option<CognitoAccountLinkError>,
    }

    impl Default for FakeLinker {
        fn default() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcome: CognitoAccountLinkOutcome::NoExistingAccount,
                failure: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl FederatedIdentityLinker for FakeLinker {
        async fn link_account(
            &self,
            user_pool_id: &str,
            source_provider_name: &str,
            source_provider_attribute_name: &str,
            source_subject: &str,
            verified_email: &str,
        ) -> Result<CognitoAccountLinkOutcome, CognitoAccountLinkError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(LinkRequest {
                    user_pool_id: user_pool_id.to_owned(),
                    source_provider_name: source_provider_name.to_owned(),
                    source_provider_attribute_name: source_provider_attribute_name.to_owned(),
                    source_subject: source_subject.to_owned(),
                    verified_email: verified_email.to_owned(),
                });
            self.failure.map_or(Ok(self.outcome), Err)
        }
    }

    fn policy(
        name: &str,
        enabled: bool,
        source_attribute: &str,
    ) -> CognitoIdentityProviderLinkingPolicy {
        CognitoIdentityProviderLinkingPolicy {
            provider_name: name.to_owned(),
            auto_link_verified_email: enabled,
            link_source_attribute_name: source_attribute.to_owned(),
        }
    }

    fn event(
        trigger: &str,
        username: Option<&str>,
        attributes: serde_json::Value,
    ) -> LambdaEvent<CognitoEventUserPoolsPreSignup> {
        let payload = serde_json::from_value(serde_json::json!({
            "version": "1",
            "triggerSource": trigger,
            "region": "eu-central-1",
            "userPoolId": "eu-central-1_test-pool",
            "userName": username,
            "callerContext": {},
            "request": { "userAttributes": attributes, "validationData": {}, "clientMetadata": {} },
            "response": {
                "autoConfirmUser": false,
                "autoVerifyEmail": false,
                "autoVerifyPhone": false
            }
        }))
        .unwrap_or_else(|error| panic!("invalid test Cognito event: {error}"));
        let mut context = Context::default();
        context.request_id = "request-test-id".to_owned();
        LambdaEvent { payload, context }
    }

    fn calls(linker: &FakeLinker) -> Vec<LinkRequest> {
        linker
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    #[tokio::test]
    async fn native_signup_is_a_noop_and_does_not_call_linker() {
        let linker = FakeLinker::default();
        let event = event("PreSignUp_SignUp", None, serde_json::json!({}));

        let result = handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await;

        assert!(result.is_ok());
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn admin_created_user_is_a_noop_and_does_not_call_linker() {
        let linker = FakeLinker::default();
        let event = event("PreSignUp_AdminCreateUser", None, serde_json::json!({}));

        let result = handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await;

        assert!(result.is_ok());
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn trusted_external_provider_is_parsed_and_forwarded_without_google_specific_logic() {
        let linker = FakeLinker::default();
        let configured = [policy("ExampleOidc", true, "Cognito_Subject")];
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("ExampleOidc_subject-opaque_42"),
            serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
        );

        let result = handler(event, &configured, &linker).await;

        assert!(result.is_ok());
        assert_eq!(
            calls(&linker),
            [LinkRequest {
                user_pool_id: "eu-central-1_test-pool".to_owned(),
                source_provider_name: "ExampleOidc".to_owned(),
                source_provider_attribute_name: "Cognito_Subject".to_owned(),
                source_subject: "subject-opaque_42".to_owned(),
                verified_email: "member@example.test".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn external_event_without_email_fails_closed() {
        let linker = FakeLinker::default();
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("Google_subject"),
            serde_json::json!({
                "email_verified": "true"
            }),
        );

        assert_eq!(
            Err(PreSignUpError::InvalidEvent),
            handler(event, &[policy("Google", true, "Cognito_Subject")], &linker)
                .await
                .map(|_| ())
        );
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn external_event_with_invalid_email_fails_closed() {
        let linker = FakeLinker::default();
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("Google_subject"),
            serde_json::json!({
                "email": "invalid-email",
                "email_verified": "true"
            }),
        );

        assert!(matches!(
            handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await,
            Err(PreSignUpError::InvalidEvent)
        ));
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn external_event_requires_verified_email_for_trusted_provider() {
        for verified in [None, Some("false")] {
            let linker = FakeLinker::default();
            let mut attributes = serde_json::json!({ "email": "member@example.test" });
            if let Some(value) = verified {
                attributes["email_verified"] = serde_json::Value::String(value.to_owned());
            }
            let event = event(
                "PreSignUp_ExternalProvider",
                Some("Google_subject"),
                attributes,
            );

            assert!(matches!(
                handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await,
                Err(PreSignUpError::InvalidEvent)
            ));
            assert!(calls(&linker).is_empty());
        }
    }

    #[tokio::test]
    async fn unknown_provider_never_receives_linking_privileges() {
        let linker = FakeLinker::default();
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("Unconfigured_subject"),
            serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
        );

        assert!(matches!(
            handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await,
            Err(PreSignUpError::InvalidEvent)
        ));
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn configured_provider_without_explicit_trust_is_a_noop_without_linking_attributes() {
        let linker = FakeLinker::default();
        for attributes in [
            serde_json::json!({}),
            serde_json::json!({ "email_verified": "false" }),
            serde_json::json!({ "email_verified": "not-a-boolean" }),
        ] {
            let event = event(
                "PreSignUp_ExternalProvider",
                Some("FutureProvider_subject"),
                attributes,
            );
            let original = event.payload.clone();

            let result = handler(
                event,
                &[policy("FutureProvider", false, "Cognito_Subject")],
                &linker,
            )
            .await;

            assert_eq!(Ok(original), result);
        }
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn overlapping_provider_names_use_longest_exact_prefix() {
        let linker = FakeLinker::default();
        let configured = [
            policy("Example", true, "UserId"),
            policy("Example_Oidc", true, "Cognito_Subject"),
        ];
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("Example_Oidc_longer-subject"),
            serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
        );

        assert!(handler(event, &configured, &linker).await.is_ok());
        let calls = calls(&linker);
        assert_eq!(calls[0].source_provider_name, "Example_Oidc");
        assert_eq!(calls[0].source_provider_attribute_name, "Cognito_Subject");
        assert_eq!(calls[0].source_subject, "longer-subject");
    }

    #[tokio::test]
    async fn malformed_provider_prefixed_username_fails_closed() {
        let linker = FakeLinker::default();
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("Google_"),
            serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
        );

        assert!(matches!(
            handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await,
            Err(PreSignUpError::InvalidEvent)
        ));
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn success_returns_original_event_payload() {
        let linker = FakeLinker::default();
        let event = event(
            "PreSignUp_ExternalProvider",
            Some("Google_subject"),
            serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
        );
        let original = event.payload.clone();

        let result = handler(event, &[policy("Google", true, "Cognito_Subject")], &linker).await;

        assert_eq!(Ok(original), result);
    }

    #[test]
    fn parses_provider_policy_and_rejects_duplicate_names() {
        let parsed = parse_linking_policy(
            r#"[{"providerName":"Google","autoLinkVerifiedEmail":true,"linkSourceAttributeName":"Cognito_Subject"}]"#,
        );
        assert_eq!(1, parsed.unwrap_or_default().len());
        assert!(parse_linking_policy(
            r#"[{"providerName":"Google","autoLinkVerifiedEmail":true,"linkSourceAttributeName":"Cognito_Subject"},{"providerName":"Google","autoLinkVerifiedEmail":false,"linkSourceAttributeName":"Cognito_Subject"}]"#
        )
        .is_err());
    }
}
