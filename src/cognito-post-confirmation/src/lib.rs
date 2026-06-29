use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use common::actor::{RequestContext, domain::Actor};
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
        .request
        .user_attributes
        .get("sub")
        .expect("shouldn't miss 'user_attribute.sub'")
        .try_into()
        .expect("shouldn't fail parsing 'user_attribute.sub' as UUID because it is a UUID according to AWS-Docs");
    let email: Email = event
        .payload
        .request
        .user_attributes
        .get("email")
        .expect("shouldn't miss user-attribute 'email' which is required by Cognito infrastructure")
        .to_owned()
        .try_into()
        .expect("shouldn't fail parsing user-attribute 'email' as valid E-Mail because Cognito forces validity on sign-up");

    let create_cmd = CreateUserCommand { id, email };
    let _ = service
        .create_user(
            &RequestContext {
                actor: Actor::User(id),
            },
            create_cmd,
        )
        .await?;

    Ok(event.payload)
}
