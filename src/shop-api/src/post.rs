use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, FORBIDDEN};
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::data::post_shop_data::PostShopData;
use shop::service::command::CreateShopCommand;
use shop::service::command_service::CommandShopService;
use user::service::user_service::UserService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    command_shop_service: &(impl CommandShopService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    user_service
        .check_admin(&user_id)
        .await
        .map_err(|_| ApiError::forbidden(FORBIDDEN))?;

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;

    let post_data: PostShopData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let create_command = CreateShopCommand {
        name: post_data.name,
        shop_type: post_data.shop_type.into(),
        domains: post_data.domains,
        image: post_data.image,
    };

    let created_shop = command_shop_service.create(create_command).await?;

    let shop_data: GetShopData = GetShopData::from(created_shop);

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .last_modified(shop_data.updated)
        .body_serde(shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use shop::core::shop::Shop;
    use shop::data::post_shop_data::PostShopData;
    use shop::service::command_service::MockCommandShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::{MockUserService, UserServiceError};

    #[tokio::test]
    async fn should_return_201_when_admin_creates_shop() {
        let admin_user_id = UserId::new();

        let mut command_service = MockCommandShopService::default();
        command_service.expect_create().return_once(move |_| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .jwt_claim("sub", admin_user_id)
                .body_serde(&Faker.fake::<PostShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &command_service, &user_service)
            .await
            .unwrap();
        assert_eq!(201, response.status_code);
    }

    #[tokio::test]
    async fn should_return_403_when_non_admin_creates_shop() {
        let user_id = UserId::new();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .jwt_claim("sub", user_id)
                .body_serde(&Faker.fake::<PostShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &user_service,
        )
        .await
        .unwrap_err();
        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_return_401_when_jwt_claim_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .body_serde(&Faker.fake::<PostShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &MockUserService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_empty() {
        let admin_user_id = UserId::new();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .jwt_claim("sub", admin_user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &user_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }
}
