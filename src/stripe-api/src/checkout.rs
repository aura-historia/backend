use crate::LiveMode;
use crate::service::StripeService;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{CONFLICT, INTERNAL_SERVER_ERROR};
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use serde::Serialize;
use url::Url;
use user::service::user_service::UserService;

#[derive(Debug, Serialize)]
pub struct CheckoutSessionResponse {
    pub url: Url,
    pub livemode: bool,
    #[serde(rename = "userId")]
    pub user_id: common::user_id::UserId,
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    stripe_service: &impl StripeService,
    user_service: &impl UserService,
    live_mode: LiveMode,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let user = user_service.find_user(&user_id).await?;

    // The user must NOT yet have an attached `stripe_customer_id` because that
    // would mean Stripe already created a customer-record for them on a
    // previous checkout. In that case the frontend should redirect to the
    // customer-portal session instead.
    if user.stripe_customer_id.is_some() {
        let err_msg =
            "User has already created a Stripe customer; use the customer-portal endpoint instead";
        return Err(ApiError::conflict(CONFLICT, err_msg.into()).with_detail(err_msg));
    }

    let url = stripe_service
        .create_checkout_session(&user_id)
        .await
        .map_err(|err| {
            let detail = err.to_string();
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                .with_detail(detail)
        })?;

    let response_data = CheckoutSessionResponse {
        url,
        livemode: live_mode.0,
        user_id,
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .body_serde(response_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::LiveMode;
    use crate::service::MockStripeService;
    use common::stripe_customer_id::StripeCustomerId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use url::Url;
    use user::core::user::User;
    use user::service::user_service::MockUserService;

    fn user_without_stripe_customer_id() -> User {
        let mut user: User = Faker.fake();
        user.stripe_customer_id = None;
        user
    }

    #[tokio::test]
    async fn should_201_with_session_url_when_user_has_no_stripe_customer_id() {
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async move { Ok(user_without_stripe_customer_id()) }));
        let mut stripe_service = MockStripeService::default();
        stripe_service
            .expect_create_checkout_session()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(Url::parse("https://checkout.stripe.com/c/pay/cs_test_123").unwrap())
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .stage("ephemeral")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &stripe_service,
            &user_service,
            LiveMode(false),
        )
        .await
        .unwrap();

        assert_eq!(201, response.status_code);
        let body = response.body.unwrap();
        let body = match body {
            aws_lambda_events::encodings::Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert!(body.contains("checkout.stripe.com"));
        assert!(body.contains("\"livemode\":false"));
        assert!(body.contains("\"userId\""));
    }

    #[tokio::test]
    async fn should_409_when_user_already_has_stripe_customer_id() {
        let mut user = Faker.fake::<User>();
        user.stripe_customer_id = Some(StripeCustomerId::from("cus_existing"));
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async move { Ok(user) }));
        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_checkout_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &stripe_service,
            &user_service,
            LiveMode(false),
        )
        .await
        .unwrap_err();

        assert_eq!(409, actual.status);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let user_service = MockUserService::default();
        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_checkout_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &stripe_service,
            &user_service,
            LiveMode(false),
        )
        .await
        .unwrap_err();

        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_serialize_livemode_true_when_stage_is_prod() {
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async move { Ok(user_without_stripe_customer_id()) }));
        let mut stripe_service = MockStripeService::default();
        stripe_service
            .expect_create_checkout_session()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(Url::parse("https://checkout.stripe.com/c/pay/cs_live_123").unwrap())
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &stripe_service, &user_service, LiveMode(true))
            .await
            .unwrap();

        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert!(body.contains("\"livemode\":true"));
    }
}
