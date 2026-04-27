use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use stripe_api::handler;
use stripe_api::service::{MockStripeService, StripePriceInfo, StripeServiceImpl};
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
        stripe_service
            .expect_get_price_by_lookup_key()
            .returning(|lookup_key| {
                let price_id = format!("price_mock_{lookup_key}");
                let info = StripePriceInfo {
                    id: price_id,
                    supported_currencies: std::collections::HashSet::new(),
                };
                Box::pin(async move { Ok(info) })
            });
        stripe_service.expect_create_checkout_session().returning(
            |user_id, _customer_id, price_id, _currency| {
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
                handler(event, &stripe_service, &user_service).await
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
                handler(event, &stripe_service, &user_service).await
            },
        ))
        .await
    }
}
