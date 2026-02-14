use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use user::data::get_user_data::GetUserAccountData;
use user::service::user_service::UserService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let user_account_data: GetUserAccountData = service.find_user(&user_id).await?.into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(user_account_data.updated)
        .cache_control("no-store", None, None)
        .body_serde(user_account_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use http::header::LAST_MODIFIED;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;
    use user::{
        core::user::User,
        service::user_service::{MockUserService, UserServiceError},
    };

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockUserService::default();
        service.expect_find_user().return_once(move |_| {
            let mut user: User = Faker.fake();
            user.updated = timestamp;
            Box::pin(async move { Ok(user) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/account")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };
        let response = handle(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_401_when_jwt_claim_sub_is_missing() {
        let mut service = MockUserService::default();
        service.expect_find_user().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/account")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_404_when_user_does_not_exist() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/account")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserService::default();
        service.expect_find_user().return_once(move |user_id| {
            let user_id = *user_id;
            Box::pin(async move { Err(UserServiceError::UserNotFound(user_id)) })
        });

        let response = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(404, response.status);
    }
}
