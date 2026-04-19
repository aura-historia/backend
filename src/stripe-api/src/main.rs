use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use stripe_api::service::{MockStripeService, StripeServiceImpl};
use stripe_api::{LiveMode, handler};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);

    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);

    let live_mode = LiveMode::from_stage(std::env::var("STAGE").ok().as_deref());

    debug!("Lambda initialized.");

    if std::env::var("LOCALSTACK_HOSTNAME").is_ok() {
        // LocalStack acceptance-tests must not perform real Stripe calls; the
        // mock returns a deterministic URL for both endpoints.
        let mut stripe_service = MockStripeService::new();
        stripe_service
            .expect_create_checkout_session()
            .returning(|user_id| {
                let url = url::Url::parse(&format!(
                    "https://checkout.stripe.com/c/pay/cs_test_{user_id}"
                ))
                .expect("shouldn't fail parsing mocked checkout URL");
                Box::pin(async move { Ok(url) })
            });
        stripe_service
            .expect_create_portal_session()
            .returning(|_, customer_id| {
                let url = url::Url::parse(&format!(
                    "https://billing.stripe.com/p/session/{customer_id}"
                ))
                .expect("shouldn't fail parsing mocked portal URL");
                Box::pin(async move { Ok(url) })
            });

        run(service_fn(
            |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
                handler(event, &stripe_service, &user_service, live_mode).await
            },
        ))
        .await
    } else {
        let api_key = std::env::var("STRIPE_API_KEY")
            .expect("shouldn't fail loading env-var 'STRIPE_API_KEY'");
        let price_id = std::env::var("STRIPE_PRO_PRICE_ID")
            .expect("shouldn't fail loading env-var 'STRIPE_PRO_PRICE_ID'");
        let frontend_base_url = std::env::var("FRONTEND_BASE_URL")
            .expect("shouldn't fail loading env-var 'FRONTEND_BASE_URL'");
        let stripe_service = StripeServiceImpl::new(api_key, price_id, frontend_base_url);

        run(service_fn(
            |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
                handler(event, &stripe_service, &user_service, live_mode).await
            },
        ))
        .await
    }
}
