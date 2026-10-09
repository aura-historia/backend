use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::cognito::{
    CognitoEventUserPoolsPostConfirmation, CognitoEventUserPoolsPostConfirmationTriggerSource,
};
use lambda_runtime::LambdaEvent;
use localization::Language;
use user_core::{first_name::FirstName, last_name::LastName};
use user_service::ports::{CognitoIdentity, CognitoIssuer, CognitoSubject};
use user_service::use_cases::{
    CognitoSignupConsent, RegisterCognitoUserCommand, RegisterCognitoUserUseCase,
};

#[derive(Debug, thiserror::Error)]
enum PostConfirmationInputError {
    #[error("missing Cognito event field: {name}")]
    MissingField { name: &'static str },
    #[error("invalid Cognito identity")]
    InvalidIdentity,
    #[error("invalid Cognito user email")]
    InvalidEmail,
}

#[tracing::instrument(
    skip(event, service),
    fields(request_id = %event.context.request_id)
)]
pub async fn handler(
    event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>,
    service: &impl RegisterCognitoUserUseCase,
) -> Result<CognitoEventUserPoolsPostConfirmation, lambda_runtime::Error> {
    let command = parse_user(&event.payload)?;
    let request_id = event.context.request_id.clone();

    service
        .execute(
            &OperationContext {
                principal: Principal::System,
                request_id: RequestId::new(request_id.clone()),
                correlation_id: CorrelationId::new(request_id),
            },
            command,
        )
        .await?;

    Ok(event.payload)
}

fn parse_user(
    event: &CognitoEventUserPoolsPostConfirmation,
) -> Result<RegisterCognitoUserCommand, PostConfirmationInputError> {
    let header = &event.cognito_event_user_pools_header;
    let region = header
        .region
        .as_deref()
        .ok_or(PostConfirmationInputError::MissingField { name: "region" })?;
    let user_pool_id = header
        .user_pool_id
        .as_deref()
        .ok_or(PostConfirmationInputError::MissingField { name: "userPoolId" })?;
    let issuer = CognitoIssuer::try_from(format!(
        "https://cognito-idp.{region}.amazonaws.com/{user_pool_id}"
    ))
    .map_err(|_| PostConfirmationInputError::InvalidIdentity)?;
    let subject = event
        .request
        .user_attributes
        .get("sub")
        .ok_or(PostConfirmationInputError::MissingField { name: "sub" })
        .and_then(|subject| {
            CognitoSubject::try_from(subject.as_str())
                .map_err(|_| PostConfirmationInputError::InvalidIdentity)
        })?;
    let email = event
        .request
        .user_attributes
        .get("email")
        .ok_or(PostConfirmationInputError::MissingField { name: "email" })?
        .as_str()
        .try_into()
        .map_err(|_| PostConfirmationInputError::InvalidEmail)?;

    let attributes = &event.request.user_attributes;
    let requested_signup_consent = parse_signup_consent(
        attributes
            .get("custom:marketing_consent")
            .map(String::as_str),
    );
    let is_native_signup = matches!(
        header.trigger_source.as_ref(),
        Some(CognitoEventUserPoolsPostConfirmationTriggerSource::ConfirmSignUp)
    );
    let email_is_verified = attributes
        .get("email_verified")
        .is_some_and(|verified| verified == "true");

    Ok(RegisterCognitoUserCommand {
        identity: CognitoIdentity { issuer, subject },
        email,
        initial_first_name: parse_first_name(attributes.get("given_name").map(String::as_str)),
        initial_last_name: parse_last_name(attributes.get("family_name").map(String::as_str)),
        initial_language: parse_language(attributes.get("locale").map(String::as_str)),
        signup_consent: if is_native_signup && email_is_verified {
            requested_signup_consent
        } else {
            None
        },
    })
}

fn parse_signup_consent(value: Option<&str>) -> Option<CognitoSignupConsent> {
    match value {
        Some("true") => Some(CognitoSignupConsent::Accepted),
        None | Some("false") => None,
        Some(_) => {
            // Do not include the event, email, or malformed attribute value in diagnostics.
            tracing::warn!(
                event = "cognito.signup_consent_attribute_invalid",
                attribute = "custom:marketing_consent",
                outcome = "ignored",
            );
            None
        }
    }
}

fn parse_first_name(value: Option<&str>) -> Option<FirstName> {
    value
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(FirstName::from)
}

fn parse_last_name(value: Option<&str>) -> Option<LastName> {
    value
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(LastName::from)
}

fn parse_language(value: Option<&str>) -> Option<Language> {
    let locale = value?.trim();
    let primary_subtag = locale.split(['-', '_']).next()?;
    if primary_subtag.is_empty() {
        return None;
    }

    Language::from_code(&primary_subtag.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{handler, parse_user};
    use application::operation_context::{OperationContext, Principal};
    use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
    use lambda_runtime::{Context, LambdaEvent};
    use localization::Language;
    use serde_email::Email;
    use std::sync::Mutex;
    use user_core::user_id::UserId;
    use user_core::{first_name::FirstName, last_name::LastName};
    use user_service::use_cases::{
        CognitoSignupConsent, RegisterCognitoUserCommand, RegisterCognitoUserError,
        RegisterCognitoUserResult, RegisterCognitoUserUseCase,
    };

    #[derive(Default)]
    struct FakeRegisterCognitoUserUseCase {
        calls: Mutex<Vec<(OperationContext, RegisterCognitoUserCommand)>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl RegisterCognitoUserUseCase for FakeRegisterCognitoUserUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: RegisterCognitoUserCommand,
        ) -> Result<RegisterCognitoUserResult, RegisterCognitoUserError> {
            if self.fail {
                return Err(RegisterCognitoUserError::BeginTransactionFailed(
                    application::error::static_error("test transaction failure"),
                ));
            }
            let result = RegisterCognitoUserResult {
                user_id: UserId::new(),
                email: command.email.clone(),
            };
            let mut calls = match self.calls.lock() {
                Ok(calls) => calls,
                Err(poisoned) => poisoned.into_inner(),
            };
            calls.push((context.clone(), command));
            Ok(result)
        }
    }

    fn event(attributes: serde_json::Value) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        let payload = match serde_json::from_value(attributes) {
            Ok(payload) => payload,
            Err(error) => panic!("invalid test Cognito event: {error}"),
        };
        let mut context = Context::default();
        context.request_id = "lambda-request-id".to_owned();
        LambdaEvent { payload, context }
    }

    fn post_confirmation_event(
        subject: &str,
        email: &str,
    ) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        post_confirmation_event_with_username("provider-username", subject, email)
    }

    fn post_confirmation_event_with_username(
        username: &str,
        subject: &str,
        email: &str,
    ) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        post_confirmation_event_with_profile(username, subject, email, None, None, None)
    }

    fn post_confirmation_event_with_profile(
        username: &str,
        subject: &str,
        email: &str,
        given_name: Option<&str>,
        family_name: Option<&str>,
        locale: Option<&str>,
    ) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        post_confirmation_event_with_signup_attributes(SignupEventFixture {
            profile: ProfileAttributes {
                given_name,
                family_name,
                locale,
            },
            ..SignupEventFixture::new(username, subject, email)
        })
    }

    #[derive(Default)]
    struct ProfileAttributes<'a> {
        given_name: Option<&'a str>,
        family_name: Option<&'a str>,
        locale: Option<&'a str>,
    }

    struct SignupEventFixture<'a> {
        username: &'a str,
        subject: &'a str,
        email: &'a str,
        profile: ProfileAttributes<'a>,
        email_verified: Option<&'a str>,
        marketing_consent: Option<&'a str>,
        trigger_source: &'a str,
    }

    impl<'a> SignupEventFixture<'a> {
        fn new(username: &'a str, subject: &'a str, email: &'a str) -> Self {
            Self {
                username,
                subject,
                email,
                profile: ProfileAttributes::default(),
                email_verified: None,
                marketing_consent: None,
                trigger_source: "PostConfirmation_ConfirmSignUp",
            }
        }
    }

    fn post_confirmation_event_with_signup_attributes(
        fixture: SignupEventFixture<'_>,
    ) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        let mut user_attributes =
            serde_json::json!({ "sub": fixture.subject, "email": fixture.email });
        for (attribute, value) in [
            ("given_name", fixture.profile.given_name),
            ("family_name", fixture.profile.family_name),
            ("locale", fixture.profile.locale),
            ("email_verified", fixture.email_verified),
            ("custom:marketing_consent", fixture.marketing_consent),
        ] {
            if let Some(value) = value {
                user_attributes[attribute] = serde_json::json!(value);
            }
        }

        event(serde_json::json!({
            "version": "1",
            "triggerSource": fixture.trigger_source,
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "userName": fixture.username,
            "callerContext": {},
            "request": {
                "userAttributes": user_attributes,
                "clientMetadata": {}
            },
            "response": {}
        }))
    }

    #[tokio::test]
    async fn should_map_opaque_cognito_identity_to_system_registration_command() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = post_confirmation_event("provider|not-a-uuid", "ada@example.com");

        let response = match handler(event, &service).await {
            Ok(response) => response,
            Err(error) => panic!("expected success: {error}"),
        };
        let calls = match service.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };

        assert_eq!(
            "provider|not-a-uuid",
            response.request.user_attributes["sub"]
        );
        assert_eq!(1, calls.len());
        assert!(matches!(calls[0].0.principal, Principal::System));
        assert_eq!("lambda-request-id", calls[0].0.request_id.as_str());
        assert_eq!("lambda-request-id", calls[0].0.correlation_id.as_str());
        assert_eq!(
            "https://cognito-idp.eu-central-1.amazonaws.com/pool-id",
            calls[0].1.identity.issuer.as_str()
        );
        assert_eq!("provider|not-a-uuid", calls[0].1.identity.subject.as_str());
        assert_eq!(email("ada@example.com"), calls[0].1.email);
    }

    #[tokio::test]
    async fn should_register_external_provider_profile_using_only_canonical_cognito_identity() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = post_confirmation_event_with_username(
            "Google_external-provider-subject",
            "canonical-cognito-subject",
            "ada@example.com",
        );

        let response = match handler(event, &service).await {
            Ok(response) => response,
            Err(error) => panic!("expected success: {error}"),
        };
        let calls = service
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        assert_eq!(
            "canonical-cognito-subject",
            response.request.user_attributes["sub"]
        );
        assert_eq!(1, calls.len());
        assert_eq!(
            "canonical-cognito-subject",
            calls[0].1.identity.subject.as_str()
        );
        assert_eq!(email("ada@example.com"), calls[0].1.email);
    }

    #[tokio::test]
    async fn should_pass_normalized_provider_profile_to_registration_service() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = post_confirmation_event_with_profile(
            "provider-username",
            "canonical-sub",
            "ada@example.com",
            Some("  Ada  "),
            Some("Mary Jane"),
            Some("en-GB"),
        );

        assert!(handler(event, &service).await.is_ok());
        let calls = service
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(1, calls.len());
        assert_eq!(Some(FirstName::from("Ada")), calls[0].1.initial_first_name);
        assert_eq!(
            Some(LastName::from("Mary Jane")),
            calls[0].1.initial_last_name
        );
        assert_eq!(Some(Language::En), calls[0].1.initial_language);
    }

    #[tokio::test]
    async fn should_succeed_for_native_registration_without_optional_profile_attributes() {
        let service = FakeRegisterCognitoUserUseCase::default();

        assert!(
            handler(
                post_confirmation_event("canonical-sub", "ada@example.com"),
                &service
            )
            .await
            .is_ok()
        );
        let calls = service
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(1, calls.len());
        assert_eq!(None, calls[0].1.initial_first_name);
        assert_eq!(None, calls[0].1.initial_last_name);
        assert_eq!(None, calls[0].1.initial_language);
    }

    #[test]
    fn should_accept_only_explicit_consent_from_verified_native_signup_attributes() {
        let accepted = post_confirmation_event_with_signup_attributes(SignupEventFixture {
            email_verified: Some("true"),
            marketing_consent: Some("true"),
            ..SignupEventFixture::new("provider-username", "canonical-sub", "ada@example.com")
        });
        assert_eq!(
            Some(CognitoSignupConsent::Accepted),
            parse_user(&accepted.payload)
                .unwrap_or_else(|error| panic!("failed to parse signup: {error}"))
                .signup_consent
        );

        for (trigger_source, email_verified, marketing_consent) in [
            ("PostConfirmation_ConfirmSignUp", Some("true"), None),
            (
                "PostConfirmation_ConfirmSignUp",
                Some("true"),
                Some("false"),
            ),
            ("PostConfirmation_ConfirmSignUp", Some("true"), Some("TRUE")),
            (
                "PostConfirmation_ConfirmSignUp",
                Some("true"),
                Some(" true"),
            ),
            (
                "PostConfirmation_ConfirmSignUp",
                Some("false"),
                Some("true"),
            ),
            ("PostConfirmation_ConfirmSignUp", None, Some("true")),
            (
                "PostConfirmation_ConfirmForgotPassword",
                Some("true"),
                Some("true"),
            ),
        ] {
            let event = post_confirmation_event_with_signup_attributes(SignupEventFixture {
                email_verified,
                marketing_consent,
                trigger_source,
                ..SignupEventFixture::new("provider-username", "canonical-sub", "ada@example.com")
            });
            assert_eq!(
                None,
                parse_user(&event.payload)
                    .unwrap_or_else(|error| panic!("failed to parse signup: {error}"))
                    .signup_consent,
                "trigger={trigger_source}, verified={email_verified:?}, consent={marketing_consent:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_ignore_consent_metadata_and_preserve_confirmation_registration() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = event(serde_json::json!({
            "version": "1",
            "triggerSource": "PostConfirmation_ConfirmSignUp",
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "userName": "provider-username",
            "callerContext": {},
            "request": {
                "userAttributes": {
                    "sub": "canonical-sub",
                    "email": "ada@example.com",
                    "email_verified": "true"
                },
                "clientMetadata": { "marketing_consent": "true" }
            },
            "response": {}
        }));

        assert!(handler(event, &service).await.is_ok());
        let calls = service
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(1, calls.len());
        assert_eq!(None, calls[0].1.signup_consent);
    }

    #[test]
    fn should_parse_partial_profile_attributes_independently() {
        let cases = [
            (
                Some("Ada"),
                None,
                None,
                Some(FirstName::from("Ada")),
                None,
                None,
            ),
            (
                None,
                Some("Lovelace"),
                None,
                None,
                Some(LastName::from("Lovelace")),
                None,
            ),
            (None, None, Some("fr-FR"), None, None, Some(Language::Fr)),
        ];

        for (given_name, family_name, locale, expected_first, expected_last, expected_language) in
            cases
        {
            let event = post_confirmation_event_with_profile(
                "provider-username",
                "canonical-sub",
                "ada@example.com",
                given_name,
                family_name,
                locale,
            );
            let command = parse_user(&event.payload)
                .unwrap_or_else(|error| panic!("failed to parse optional profile: {error}"));

            assert_eq!(expected_first, command.initial_first_name);
            assert_eq!(expected_last, command.initial_last_name);
            assert_eq!(expected_language, command.initial_language);
        }
    }

    #[tokio::test]
    async fn should_succeed_with_empty_names_and_unsupported_locale() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = post_confirmation_event_with_profile(
            "provider-username",
            "canonical-sub",
            "ada@example.com",
            Some("   "),
            Some(""),
            Some("xx-YY"),
        );

        assert!(handler(event, &service).await.is_ok());
        let calls = service
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(1, calls.len());
        assert_eq!(None, calls[0].1.initial_first_name);
        assert_eq!(None, calls[0].1.initial_last_name);
        assert_eq!(None, calls[0].1.initial_language);
    }

    #[test]
    fn should_reduce_supported_locale_variants_to_primary_language() {
        for (locale, expected) in [
            ("de", Some(Language::De)),
            ("de-DE", Some(Language::De)),
            ("DE-de", Some(Language::De)),
            ("en-US", Some(Language::En)),
            ("en_US", Some(Language::En)),
            ("EN_us", Some(Language::En)),
            ("pt-BR", Some(Language::Pt)),
            ("zh-CN", Some(Language::Zh)),
            ("zh-Hans", Some(Language::Zh)),
            ("", None),
            ("  ", None),
            ("xx-YY", None),
            ("-en", None),
        ] {
            assert_eq!(expected, super::parse_language(Some(locale)));
        }
        assert_eq!(None, super::parse_language(None));
    }

    #[tokio::test]
    async fn should_fail_without_calling_service_when_required_identity_field_is_missing() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = event(serde_json::json!({
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "callerContext": {},
            "request": { "userAttributes": { "email": "ada@example.com" } },
            "response": {}
        }));

        assert!(handler(event, &service).await.is_err());
        let calls = match service.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_service_error_for_cognito_retry() {
        let service = FakeRegisterCognitoUserUseCase {
            fail: true,
            ..Default::default()
        };

        assert!(
            handler(
                post_confirmation_event("provider|opaque-subject", "ada@example.com"),
                &service
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn should_reject_missing_or_invalid_user_attributes() {
        let missing_pool: CognitoEventUserPoolsPostConfirmation =
            match serde_json::from_value(serde_json::json!({
                "region": "eu-central-1",
                "callerContext": {},
                "request": {
                    "userAttributes": { "sub": "opaque", "email": "ada@example.com" }
                },
                "response": {}
            })) {
                Ok(event) => event,
                Err(error) => panic!("invalid test Cognito event: {error}"),
            };
        let missing_email = event(serde_json::json!({
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "callerContext": {},
            "request": { "userAttributes": { "sub": "opaque" } },
            "response": {}
        }));
        let invalid_sub = post_confirmation_event("", "ada@example.com");
        let invalid_email = post_confirmation_event("opaque", "invalid");

        assert!(parse_user(&missing_pool).is_err());
        assert!(parse_user(&missing_email.payload).is_err());
        assert!(parse_user(&invalid_sub.payload).is_err());
        assert!(parse_user(&invalid_email.payload).is_err());
    }

    fn email(value: &str) -> Email {
        match value.try_into() {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }
}
