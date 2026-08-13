use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use common::postgres::{SqlxUnitOfWork, connect_from_env};
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use user_postgres::SqlxUserRepositoryFactory;
use user_service::use_cases::CreateUserHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let pool = connect_from_env().await?;
    let service =
        CreateUserHandler::new(SqlxUnitOfWork::new(pool), SqlxUserRepositoryFactory::new());

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| async {
            handler(event, &service).await
        },
    ))
    .await
}
