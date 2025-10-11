use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use lambda_runtime::LambdaEvent;
use user_service::service::UserService;

#[tracing::instrument(
    skip(event, service),
    fields(requestId = %event.context.request_id,)
)]
pub async fn handler(
    event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>,
    service: &impl UserService,
) -> Result<CognitoEventUserPoolsPostConfirmation, lambda_runtime::Error> {
    unimplemented!()
}
