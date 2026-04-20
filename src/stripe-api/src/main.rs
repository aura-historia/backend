use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use std::collections::HashMap;
use stripe_api::handler;
use stripe_api::service::{MockStripeService, StripeServiceImpl};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

const PRICE_ID_ENV_VARS: [&str; 4] = [
    "STRIPE_PRO_MONTHLY_PRICE_ID",
    "STRIPE_PRO_YEARLY_PRICE_ID",
    "STRIPE_ULTIMATE_MONTHLY_PRICE_ID",
    "STRIPE_ULTIMATE_YEARLY_PRICE_ID",
];

fn load_price_ids() -> HashMap<&'static str, String> {
    PRICE_ID_ENV_VARS
        .iter()
        .map(|name| {
            let value = std::env::var(name)
                .unwrap_or_else(|_| panic!("shouldn't fail loading env-var '{name}'"));
            (*name, value)
        })
        .collect()
}

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

    let price_ids = load_price_ids();

    debug!("Lambda initialized.");

    if std::env::var("LOCALSTACK_HOSTNAME").is_ok() {
        // LocalStack acceptance-tests must not perform real Stripe calls; the
        // mock returns deterministic URLs for both endpoints and a
        // deterministic customer-id for the create-customer call.
        let mut stripe_service = MockStripeService::new();
        stripe_service.expect_create_customer().returning(|req| {
            let id = common::stripe_customer_id::StripeCustomerId::from(format!(
                "cus_mocked_{}",
                req.user_id
            ));
            Box::pin(async move { Ok(id) })
        });
        stripe_service.expect_create_checkout_session().returning(
            |user_id, _customer_id, price_id| {
                let url = url::Url::parse(&format!(
                    "https://checkout.stripe.com/c/pay/cs_test_{user_id}_{price_id}"
                ))
                .expect("shouldn't fail parsing mocked checkout URL");
                Box::pin(async move { Ok(url) })
            },
        );
        stripe_service
            .expect_create_portal_session()
            .returning(|customer_id| {
                let url = url::Url::parse(&format!(
                    "https://billing.stripe.com/p/session/{customer_id}"
                ))
                .expect("shouldn't fail parsing mocked portal URL");
                Box::pin(async move { Ok(url) })
            });

        run(service_fn(
            |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
                handler(event, &stripe_service, &user_service, &price_ids).await
            },
        ))
        .await
    } else {
        let api_key = std::env::var("STRIPE_API_KEY")
            .expect("shouldn't fail loading env-var 'STRIPE_API_KEY'");
        let checkout_success_url = std::env::var("STRIPE_CHECKOUT_SUCCESS_URL")
            .expect("shouldn't fail loading env-var 'STRIPE_CHECKOUT_SUCCESS_URL'");
        let checkout_cancel_url = std::env::var("STRIPE_CHECKOUT_CANCEL_URL")
            .expect("shouldn't fail loading env-var 'STRIPE_CHECKOUT_CANCEL_URL'");
        let portal_return_url = std::env::var("STRIPE_PORTAL_RETURN_URL")
            .expect("shouldn't fail loading env-var 'STRIPE_PORTAL_RETURN_URL'");
        let stripe_service = StripeServiceImpl::new(
            api_key,
            checkout_success_url,
            checkout_cancel_url,
            portal_return_url,
        );

        run(service_fn(
            |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
                handler(event, &stripe_service, &user_service, &price_ids).await
            },
        ))
        .await
    }
}
