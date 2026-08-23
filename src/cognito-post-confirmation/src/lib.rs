use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use lambda_runtime::LambdaEvent;
use serde_email::Email;
use user_core::user_id::UserId;
use user_service::use_cases::{CreateUserCommand, CreateUserUseCase};

#[derive(Debug, thiserror::Error)]
enum PostConfirmationInputError {
    #[error("missing Cognito user attribute: {name}")]
    MissingAttribute { name: &'static str },
    #[error("invalid Cognito user identifier")]
    InvalidUserId,
    #[error("invalid Cognito user email")]
    InvalidEmail,
}

#[tracing::instrument(
    skip(event, service),
    fields(request_id = %event.context.request_id)
)]
pub async fn handler(
    event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>,
    service: &impl CreateUserUseCase,
) -> Result<CognitoEventUserPoolsPostConfirmation, lambda_runtime::Error> {
    let (user_id, email) = parse_user(&event.payload)?;
    let request_id = event.context.request_id.clone();

    service
        .execute(
            &OperationContext {
                principal: Principal::System,
                request_id: RequestId::new(request_id.clone()),
                correlation_id: CorrelationId::new(request_id),
            },
            CreateUserCommand { user_id, email },
        )
        .await?;

    Ok(event.payload)
}

fn parse_user(
    event: &CognitoEventUserPoolsPostConfirmation,
) -> Result<(UserId, Email), PostConfirmationInputError> {
    let user_id = event
        .request
        .user_attributes
        .get("sub")
        .ok_or(PostConfirmationInputError::MissingAttribute { name: "sub" })?
        .try_into()
        .map_err(|_| PostConfirmationInputError::InvalidUserId)?;
    let email = event
        .request
        .user_attributes
        .get("email")
        .ok_or(PostConfirmationInputError::MissingAttribute { name: "email" })?
        .as_str()
        .try_into()
        .map_err(|_| PostConfirmationInputError::InvalidEmail)?;

    Ok((user_id, email))
}

#[cfg(test)]
mod tests {
    use super::{handler, parse_user};
    use application::operation_context::{OperationContext, Principal};
    use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
    use lambda_runtime::{Context, LambdaEvent};
    use serde_email::Email;
    use std::sync::Mutex;
    use user_core::user_id::UserId;
    use user_service::use_cases::{
        CreateUserCommand, CreateUserError, CreateUserResult, CreateUserUseCase,
    };

    #[derive(Default)]
    struct FakeCreateUserUseCase {
        calls: Mutex<Vec<(OperationContext, CreateUserCommand)>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl CreateUserUseCase for FakeCreateUserUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: CreateUserCommand,
        ) -> Result<CreateUserResult, CreateUserError> {
            if self.fail {
                return Err(CreateUserError::BeginTransactionFailed);
            }
            let result = CreateUserResult {
                user_id: command.user_id,
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
        user_id: UserId,
        email: &str,
    ) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        event(serde_json::json!({
            "version": "1",
            "triggerSource": "PostConfirmation_ConfirmSignUp",
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "userName": user_id.to_string(),
            "callerContext": {},
            "request": {
                "userAttributes": {
                    "sub": user_id.to_string(),
                    "email": email
                },
                "clientMetadata": {}
            },
            "response": {}
        }))
    }

    #[tokio::test]
    async fn should_map_cognito_attributes_to_system_create_user_command() {
        let user_id = UserId::new();
        let service = FakeCreateUserUseCase::default();
        let event = post_confirmation_event(user_id, "ada@example.com");

        let response = match handler(event, &service).await {
            Ok(response) => response,
            Err(error) => panic!("expected success: {error}"),
        };
        let calls = match service.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };

        assert_eq!(user_id.to_string(), response.request.user_attributes["sub"]);
        assert_eq!(1, calls.len());
        assert!(matches!(calls[0].0.principal, Principal::System));
        assert_eq!("lambda-request-id", calls[0].0.request_id.as_str());
        assert_eq!("lambda-request-id", calls[0].0.correlation_id.as_str());
        assert_eq!(user_id, calls[0].1.user_id);
        assert_eq!(email("ada@example.com"), calls[0].1.email);
    }

    #[tokio::test]
    async fn should_fail_without_calling_service_when_required_attribute_is_invalid() {
        let service = FakeCreateUserUseCase::default();
        let event = event(serde_json::json!({
            "callerContext": {},
            "request": { "userAttributes": { "sub": "not-a-uuid" } },
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
        let service = FakeCreateUserUseCase {
            fail: true,
            ..Default::default()
        };

        assert!(
            handler(
                post_confirmation_event(UserId::new(), "ada@example.com"),
                &service
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn should_reject_missing_or_invalid_user_attributes() {
        let missing_sub: CognitoEventUserPoolsPostConfirmation =
            match serde_json::from_value(serde_json::json!({
                "callerContext": {},
                "request": { "userAttributes": { "email": "ada@example.com" } },
                "response": {}
            })) {
                Ok(event) => event,
                Err(error) => panic!("invalid test Cognito event: {error}"),
            };
        let invalid_email: CognitoEventUserPoolsPostConfirmation =
            match serde_json::from_value(serde_json::json!({
                "callerContext": {},
                "request": {
                    "userAttributes": { "sub": UserId::new().to_string(), "email": "invalid" }
                },
                "response": {}
            })) {
                Ok(event) => event,
                Err(error) => panic!("invalid test Cognito event: {error}"),
            };

        assert!(parse_user(&missing_sub).is_err());
        assert!(parse_user(&invalid_email).is_err());
    }

    fn email(value: &str) -> Email {
        match value.try_into() {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }
}
