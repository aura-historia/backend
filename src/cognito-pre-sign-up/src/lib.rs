use aws_lambda_events::cognito::{
    CognitoEventUserPoolsPreSignup, CognitoEventUserPoolsPreSignupTriggerSource,
};
use lambda_runtime::LambdaEvent;
use serde_email::Email;
use std::{collections::HashSet, time::Instant};
use user_cognito::{CognitoAccountLinkError, CognitoAccountLinker};
use user_service::{
    ports::{CognitoIssuer, CognitoSubject},
    use_cases::{
        ExistingEmailAction, FederatedAccountDecision, ResolveFederatedAccountCommand,
        ResolveFederatedAccountError, ResolveFederatedAccountUseCase,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CognitoIdentityProviderPreSignUpPolicy {
    pub provider_name: String,
    pub existing_email_action: ExistingEmailAction,
    pub link_source_attribute_name: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PreSignUpError {
    #[error("invalid Cognito pre-sign-up policy")]
    InvalidPolicy,
    #[error("invalid external-provider sign-up event")]
    InvalidEvent,
    #[error("Cognito account lookup is temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("invalid Cognito account-link state")]
    InvalidIdentityState,
    #[error("an account already exists for this email")]
    ExistingAccount,
}

impl PreSignUpError {
    pub const fn category(&self) -> &'static str {
        match self {
            Self::InvalidPolicy => "invalid_policy",
            Self::InvalidEvent => "invalid_event",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::InvalidIdentityState => "invalid_identity_state",
            Self::ExistingAccount => "existing_account",
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
        destination_subject: &CognitoSubject,
        canonical_email: &str,
    ) -> Result<(), CognitoAccountLinkError>;
}

#[async_trait::async_trait]
impl FederatedIdentityLinker for CognitoAccountLinker {
    async fn link_account(
        &self,
        user_pool_id: &str,
        source_provider_name: &str,
        source_provider_attribute_name: &str,
        source_subject: &str,
        destination_subject: &CognitoSubject,
        canonical_email: &str,
    ) -> Result<(), CognitoAccountLinkError> {
        CognitoAccountLinker::link_account(
            self,
            user_pool_id,
            source_provider_name,
            source_provider_attribute_name,
            source_subject,
            destination_subject,
            canonical_email,
        )
        .await
    }
}

pub fn parse_provider_signup_policy(
    value: &str,
) -> Result<Vec<CognitoIdentityProviderPreSignUpPolicy>, PreSignUpError> {
    let value: serde_json::Value =
        serde_json::from_str(value).map_err(|_| PreSignUpError::InvalidPolicy)?;
    let entries = value.as_array().ok_or(PreSignUpError::InvalidPolicy)?;
    let mut names = HashSet::with_capacity(entries.len());
    let mut policies = Vec::with_capacity(entries.len());

    for entry in entries {
        let object = entry.as_object().ok_or(PreSignUpError::InvalidPolicy)?;
        let provider_name = entry
            .get("providerName")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty() && name.trim() == *name)
            .ok_or(PreSignUpError::InvalidPolicy)?;
        if !names.insert(provider_name.to_owned()) {
            return Err(PreSignUpError::InvalidPolicy);
        }

        let action = entry
            .get("existingEmailAction")
            .and_then(serde_json::Value::as_str)
            .ok_or(PreSignUpError::InvalidPolicy)?;
        let (existing_email_action, link_source_attribute_name) = match action {
            "LINK_VERIFIED" => {
                let attribute = entry
                    .get("linkSourceAttributeName")
                    .and_then(serde_json::Value::as_str)
                    .filter(|attribute| *attribute == "Cognito_Subject")
                    .ok_or(PreSignUpError::InvalidPolicy)?;
                (
                    ExistingEmailAction::LinkVerified,
                    Some(attribute.to_owned()),
                )
            }
            "REJECT" => {
                if entry.get("linkSourceAttributeName").is_some() {
                    return Err(PreSignUpError::InvalidPolicy);
                }
                (ExistingEmailAction::Reject, None)
            }
            _ => return Err(PreSignUpError::InvalidPolicy),
        };

        let allowed_keys = [
            "providerName",
            "existingEmailAction",
            "linkSourceAttributeName",
        ];
        if object
            .keys()
            .any(|key| !allowed_keys.contains(&key.as_str()))
        {
            return Err(PreSignUpError::InvalidPolicy);
        }
        policies.push(CognitoIdentityProviderPreSignUpPolicy {
            provider_name: provider_name.to_owned(),
            existing_email_action,
            link_source_attribute_name,
        });
    }
    Ok(policies)
}

pub fn is_external_provider_signup(event: &CognitoEventUserPoolsPreSignup) -> bool {
    matches!(
        event
            .cognito_event_user_pools_header
            .trigger_source
            .as_ref(),
        Some(CognitoEventUserPoolsPreSignupTriggerSource::ExternalProvider)
    )
}

#[tracing::instrument(skip(event, providers, resolver, linker), fields(request_id = %event.context.request_id))]
pub async fn handler<R: ResolveFederatedAccountUseCase, L: FederatedIdentityLinker>(
    event: LambdaEvent<CognitoEventUserPoolsPreSignup>,
    providers: &[CognitoIdentityProviderPreSignUpPolicy],
    resolver: &R,
    linker: &L,
) -> Result<CognitoEventUserPoolsPreSignup, PreSignUpError> {
    let started_at = Instant::now();
    let request_id = event.context.request_id.clone();
    let payload = event.payload;
    if !is_external_provider_signup(&payload) {
        tracing::info!(
            request_id,
            trigger_kind = "non_external",
            result_category = "no_op",
        );
        return Ok(payload);
    }

    let header = &payload.cognito_event_user_pools_header;
    let username = header
        .user_name
        .as_deref()
        .ok_or(PreSignUpError::InvalidEvent)?;
    let (provider, source_subject) = resolve_provider(username, providers)?;

    let email_value = payload
        .request
        .user_attributes
        .get("email")
        .ok_or(PreSignUpError::InvalidEvent)?;
    let email: Email = email_value
        .as_str()
        .try_into()
        .map_err(|_| PreSignUpError::InvalidEvent)?;
    let email_verified = payload
        .request
        .user_attributes
        .get("email_verified")
        .is_some_and(|verified| verified == "true");

    let user_pool_id = header
        .user_pool_id
        .as_deref()
        .filter(|id| !id.trim().is_empty() && id.trim() == *id)
        .ok_or(PreSignUpError::InvalidEvent)?;
    let region = header
        .region
        .as_deref()
        .filter(|region| !region.trim().is_empty() && region.trim() == *region)
        .ok_or(PreSignUpError::InvalidEvent)?;
    let issuer = CognitoIssuer::try_from(format!(
        "https://cognito-idp.{region}.amazonaws.com/{user_pool_id}"
    ))
    .map_err(|_| PreSignUpError::InvalidEvent)?;

    let decision = resolver
        .execute(ResolveFederatedAccountCommand {
            email: email.clone(),
            email_verified,
            current_issuer: issuer.clone(),
            existing_email_action: provider.existing_email_action,
        })
        .await
        .map_err(map_resolver_error)?;

    match decision {
        FederatedAccountDecision::AllowNewAccount => {
            tracing::info!(
                request_id,
                trigger_kind = "external_provider",
                provider_name = %provider.provider_name,
                result_category = "no_email_collision",
                duration_ms = started_at.elapsed().as_millis() as u64,
            );
            Ok(payload)
        }
        FederatedAccountDecision::RejectExistingAccount => {
            tracing::info!(
                request_id,
                trigger_kind = "external_provider",
                provider_name = %provider.provider_name,
                result_category = "existing_email_rejected",
                duration_ms = started_at.elapsed().as_millis() as u64,
            );
            Err(PreSignUpError::ExistingAccount)
        }
        FederatedAccountDecision::LinkExistingAccount {
            identity,
            canonical_email,
        } => {
            if provider.existing_email_action != ExistingEmailAction::LinkVerified
                || !email_verified
                || identity.issuer != issuer
            {
                return Err(PreSignUpError::InvalidIdentityState);
            }
            let source_attribute_name = provider
                .link_source_attribute_name
                .as_deref()
                .ok_or(PreSignUpError::InvalidPolicy)?;
            linker
                .link_account(
                    user_pool_id,
                    &provider.provider_name,
                    source_attribute_name,
                    source_subject,
                    &identity.subject,
                    canonical_email.as_ref(),
                )
                .await
                .map_err(map_link_error)?;
            tracing::info!(
                request_id,
                trigger_kind = "external_provider",
                provider_name = %provider.provider_name,
                result_category = "linked_to_postgres_identity",
                duration_ms = started_at.elapsed().as_millis() as u64,
            );
            Ok(payload)
        }
    }
}

fn map_resolver_error(error: ResolveFederatedAccountError) -> PreSignUpError {
    match error {
        ResolveFederatedAccountError::TemporarilyUnavailable => {
            PreSignUpError::TemporarilyUnavailable
        }
        ResolveFederatedAccountError::InvalidPersistedState => PreSignUpError::InvalidIdentityState,
    }
}

fn map_link_error(error: CognitoAccountLinkError) -> PreSignUpError {
    match error {
        CognitoAccountLinkError::TemporarilyUnavailable => PreSignUpError::TemporarilyUnavailable,
        CognitoAccountLinkError::InvalidState => PreSignUpError::InvalidIdentityState,
    }
}

fn resolve_provider<'a>(
    username: &'a str,
    providers: &'a [CognitoIdentityProviderPreSignUpPolicy],
) -> Result<(&'a CognitoIdentityProviderPreSignUpPolicy, &'a str), PreSignUpError> {
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
    if longest_matches.next().is_some()
        || matched.1.trim().is_empty()
        || matched.1.chars().any(char::is_control)
        || matched.1.len() > 2_048
    {
        return Err(PreSignUpError::InvalidEvent);
    }
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::{Context, LambdaEvent};
    use std::sync::Mutex;
    use user_service::{
        ports::{CognitoIdentity, CognitoSubject},
        use_cases::{
            FederatedAccountDecision, ResolveFederatedAccountCommand, ResolveFederatedAccountError,
        },
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct LinkRequest {
        user_pool_id: String,
        source_provider_name: String,
        source_provider_attribute_name: String,
        source_subject: String,
        destination_subject: String,
        canonical_email: String,
    }

    struct FakeLinker {
        calls: Mutex<Vec<LinkRequest>>,
        failure: Option<CognitoAccountLinkError>,
    }

    impl Default for FakeLinker {
        fn default() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
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
            destination_subject: &CognitoSubject,
            canonical_email: &str,
        ) -> Result<(), CognitoAccountLinkError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(LinkRequest {
                    user_pool_id: user_pool_id.to_owned(),
                    source_provider_name: source_provider_name.to_owned(),
                    source_provider_attribute_name: source_provider_attribute_name.to_owned(),
                    source_subject: source_subject.to_owned(),
                    destination_subject: destination_subject.as_str().to_owned(),
                    canonical_email: canonical_email.to_owned(),
                });
            self.failure.map_or(Ok(()), Err)
        }
    }

    struct FakeResolver {
        decisions: Mutex<Vec<FederatedAccountDecision>>,
        commands: Mutex<Vec<ResolveFederatedAccountCommand>>,
        failure: Option<ResolveFederatedAccountError>,
    }

    impl FakeResolver {
        fn new(decision: FederatedAccountDecision) -> Self {
            Self {
                decisions: Mutex::new(vec![decision]),
                commands: Mutex::new(Vec::new()),
                failure: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl ResolveFederatedAccountUseCase for FakeResolver {
        async fn execute(
            &self,
            command: ResolveFederatedAccountCommand,
        ) -> Result<FederatedAccountDecision, ResolveFederatedAccountError> {
            self.commands
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(command);
            if let Some(error) = &self.failure {
                return Err(error.clone());
            }
            let mut decisions = self
                .decisions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if decisions.is_empty() {
                return Err(ResolveFederatedAccountError::InvalidPersistedState);
            }
            Ok(decisions.remove(0))
        }
    }

    fn policy(
        name: &str,
        action: ExistingEmailAction,
        source_attribute: Option<&str>,
    ) -> CognitoIdentityProviderPreSignUpPolicy {
        CognitoIdentityProviderPreSignUpPolicy {
            provider_name: name.to_owned(),
            existing_email_action: action,
            link_source_attribute_name: source_attribute.map(str::to_owned),
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

    fn identity(subject: &str) -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from(
                "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_test-pool",
            )
            .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
            subject: CognitoSubject::try_from(subject)
                .unwrap_or_else(|error| panic!("invalid test subject: {error}")),
        }
    }

    fn calls(linker: &FakeLinker) -> Vec<LinkRequest> {
        linker
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    #[tokio::test]
    async fn native_and_admin_created_users_are_noops() {
        let resolver = FakeResolver::new(FederatedAccountDecision::AllowNewAccount);
        let linker = FakeLinker::default();
        for trigger in ["PreSignUp_SignUp", "PreSignUp_AdminCreateUser"] {
            let result = handler(
                event(trigger, None, serde_json::json!({})),
                &[policy(
                    "Google",
                    ExistingEmailAction::LinkVerified,
                    Some("Cognito_Subject"),
                )],
                &resolver,
                &linker,
            )
            .await;
            assert!(result.is_ok());
        }
        assert!(
            resolver
                .commands
                .lock()
                .expect("resolver commands")
                .is_empty()
        );
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn no_match_allows_google_even_when_provider_email_is_unverified() {
        let resolver = FakeResolver::new(FederatedAccountDecision::AllowNewAccount);
        let linker = FakeLinker::default();
        let result = handler(
            event(
                "PreSignUp_ExternalProvider",
                Some("Google_subject-opaque"),
                serde_json::json!({ "email": "member@example.test", "email_verified": "false" }),
            ),
            &[policy(
                "Google",
                ExistingEmailAction::LinkVerified,
                Some("Cognito_Subject"),
            )],
            &resolver,
            &linker,
        )
        .await;

        assert!(result.is_ok());
        let commands = resolver.commands.lock().expect("resolver commands");
        assert_eq!(1, commands.len());
        assert!(!commands[0].email_verified);
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn no_match_allows_facebook_without_marking_email_verified() {
        let resolver = FakeResolver::new(FederatedAccountDecision::AllowNewAccount);
        let linker = FakeLinker::default();
        let result = handler(
            event(
                "PreSignUp_ExternalProvider",
                Some("Facebook_subject-opaque"),
                serde_json::json!({ "email": "member@example.test" }),
            ),
            &[policy("Facebook", ExistingEmailAction::Reject, None)],
            &resolver,
            &linker,
        )
        .await;

        assert!(result.is_ok());
        let commands = resolver.commands.lock().expect("resolver commands");
        assert_eq!(
            ExistingEmailAction::Reject,
            commands[0].existing_email_action
        );
        assert!(!commands[0].email_verified);
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn existing_facebook_email_is_rejected_without_cognito_linking() {
        let resolver = FakeResolver::new(FederatedAccountDecision::RejectExistingAccount);
        let linker = FakeLinker::default();
        let result = handler(
            event(
                "PreSignUp_ExternalProvider",
                Some("Facebook_subject-opaque"),
                serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
            ),
            &[policy("Facebook", ExistingEmailAction::Reject, None)],
            &resolver,
            &linker,
        )
        .await;

        assert_eq!(Err(PreSignUpError::ExistingAccount), result.map(|_| ()));
        assert!(calls(&linker).is_empty());
    }

    #[tokio::test]
    async fn google_link_uses_postgres_bound_subject_and_canonical_email() {
        let resolver = FakeResolver::new(FederatedAccountDecision::LinkExistingAccount {
            identity: identity("canonical-sub"),
            canonical_email: "member@example.test"
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
        });
        let linker = FakeLinker::default();
        let result = handler(
            event(
                "PreSignUp_ExternalProvider",
                Some("Google_google-subject"),
                serde_json::json!({ "email": "member@example.test", "email_verified": "true" }),
            ),
            &[policy(
                "Google",
                ExistingEmailAction::LinkVerified,
                Some("Cognito_Subject"),
            )],
            &resolver,
            &linker,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            calls(&linker),
            [LinkRequest {
                user_pool_id: "eu-central-1_test-pool".to_owned(),
                source_provider_name: "Google".to_owned(),
                source_provider_attribute_name: "Cognito_Subject".to_owned(),
                source_subject: "google-subject".to_owned(),
                destination_subject: "canonical-sub".to_owned(),
                canonical_email: "member@example.test".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn malformed_event_and_database_errors_fail_closed() {
        let resolver = FakeResolver::new(FederatedAccountDecision::AllowNewAccount);
        let linker = FakeLinker::default();
        for attributes in [
            serde_json::json!({}),
            serde_json::json!({ "email": "invalid-email" }),
        ] {
            assert!(matches!(
                handler(
                    event(
                        "PreSignUp_ExternalProvider",
                        Some("Google_subject"),
                        attributes
                    ),
                    &[policy(
                        "Google",
                        ExistingEmailAction::LinkVerified,
                        Some("Cognito_Subject")
                    )],
                    &resolver,
                    &linker,
                )
                .await,
                Err(PreSignUpError::InvalidEvent)
            ));
        }
        let unavailable = FakeResolver {
            decisions: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            failure: Some(ResolveFederatedAccountError::TemporarilyUnavailable),
        };
        assert!(matches!(
            handler(
                event(
                    "PreSignUp_ExternalProvider",
                    Some("Facebook_subject"),
                    serde_json::json!({ "email": "member@example.test" }),
                ),
                &[policy("Facebook", ExistingEmailAction::Reject, None)],
                &unavailable,
                &linker,
            )
            .await,
            Err(PreSignUpError::TemporarilyUnavailable)
        ));
    }

    #[test]
    fn parses_discriminated_provider_policy_and_rejects_contradictory_entries() {
        let parsed = parse_provider_signup_policy(
            r#"[{"providerName":"Google","existingEmailAction":"LINK_VERIFIED","linkSourceAttributeName":"Cognito_Subject"},{"providerName":"Facebook","existingEmailAction":"REJECT"}]"#,
        )
        .unwrap_or_default();
        assert_eq!(2, parsed.len());
        assert!(parse_provider_signup_policy(
            r#"[{"providerName":"Facebook","existingEmailAction":"REJECT","linkSourceAttributeName":"Cognito_Subject"}]"#
        )
        .is_err());
        assert!(
            parse_provider_signup_policy(
                r#"[{"providerName":"Google","existingEmailAction":"LINK_VERIFIED"}]"#
            )
            .is_err()
        );
        assert!(parse_provider_signup_policy(
            r#"[{"providerName":"Google","existingEmailAction":"LINK_VERIFIED","linkSourceAttributeName":"email"}]"#
        )
        .is_err());
        assert!(parse_provider_signup_policy(
            r#"[{"providerName":"Google","existingEmailAction":"REJECT"},{"providerName":"Google","existingEmailAction":"REJECT"}]"#
        )
        .is_err());
    }
}
