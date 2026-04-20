use crate::service::StripeService;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{INTERNAL_SERVER_ERROR, STRIPE_CUSTOMER_DOES_NOT_EXIST};
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use serde::Serialize;
use url::Url;
use user::service::user_service::UserService;

#[derive(Debug, Serialize)]
pub struct PortalSessionResponse {
    pub url: Url,
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    stripe_service: &impl StripeService,
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let user = user_service.find_user(&user_id).await?;

    // The user MUST have a `stripe_customer_id` because the customer-portal
    // is scoped to existing customers; without one there is nothing to
    // manage.
    let stripe_customer_id = user.stripe_customer_id.as_ref().ok_or_else(|| {
        let err_msg =
            "User has never had a Stripe subscription; no customer-portal session can be created";
        ApiError::unprocessable_entity(STRIPE_CUSTOMER_DOES_NOT_EXIST, err_msg.into())
            .with_detail(err_msg)
    })?;

    let url = stripe_service
        .create_portal_session(stripe_customer_id)
        .await
        .map_err(|err| {
            let detail = err.to_string();
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                .with_detail(detail)
        })?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .body_serde(PortalSessionResponse { url })?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::service::MockStripeService;
    use common::api::error_code::STRIPE_CUSTOMER_DOES_NOT_EXIST;
    use common::stripe_customer_id::StripeCustomerId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use url::Url;
    use user::core::user::User;
    use user::service::user_service::MockUserService;

    fn user_with_stripe_customer_id() -> User {
        let mut user: User = Faker.fake();
        user.stripe_customer_id = Some(StripeCustomerId::from("cus_test_123"));
        user
    }

    #[tokio::test]
    async fn should_201_with_portal_url_when_user_has_stripe_customer_id() {
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async move { Ok(user_with_stripe_customer_id()) }));
        let mut stripe_service = MockStripeService::default();
        stripe_service
            .expect_create_portal_session()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(Url::parse("https://billing.stripe.com/p/session/test_123").unwrap())
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &stripe_service, &user_service)
            .await
            .unwrap();

        assert_eq!(201, response.status_code);
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert!(body.contains("billing.stripe.com"));
        assert!(!body.contains("livemode"));
        assert!(!body.contains("userId"));
    }

    #[tokio::test]
    async fn should_422_with_dedicated_error_code_when_user_has_no_stripe_customer_id() {
        let mut user = Faker.fake::<User>();
        user.stripe_customer_id = None;
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async move { Ok(user) }));
        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &stripe_service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(422, actual.status);
        assert_eq!(STRIPE_CUSTOMER_DOES_NOT_EXIST, actual.error);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let user_service = MockUserService::default();
        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &stripe_service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(401, actual.status);
    }
}
