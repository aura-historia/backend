use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use common::user_id::UserId;
use lambda_runtime::LambdaEvent;
use serde_email::Email;
use user::service::{command::CreateUserCommand, user_service::UserService};

#[tracing::instrument(
    skip(event, service),
    fields(requestId = %event.context.request_id,)
)]
pub async fn handler(
    event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>,
    service: &impl UserService,
) -> Result<CognitoEventUserPoolsPostConfirmation, lambda_runtime::Error> {
    let id: UserId = event
        .payload
        .cognito_event_user_pools_header
        .user_name
        .as_deref()
        .expect("shouldn't miss 'user_name' which actually is the 'sub' and therefore required according to AWS-Docs")
        .try_into()
        .expect("shouldn't fail parsing 'user_name' aka 'sub' as UUID because it is a UUID according to AWS-Docs");
    let email: Email = event
        .payload
        .request
        .user_attributes
        .get("email")
        .expect("shouldn't miss user-attribute 'email' which is required according to our Cloudformation-Code for Cognito")
        .to_owned()
        .try_into()
        .expect("shouldn't fail parsing user-attribute 'email' as valid E-Mail because Cognito forces validity on sign-up");

    let create_cmd = CreateUserCommand { id, email };
    let _ = service.create_user(create_cmd).await?;

    Ok(event.payload)
}
