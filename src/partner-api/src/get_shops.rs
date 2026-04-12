use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID};
use common::error::missing_field::MissingRequiredField;
use common::user_id::UserId;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::service::get_service::GetShopService;
use std::collections::HashMap;
use user::service::user_service::UserService;

fn extract_partner_id_path(path_params: &HashMap<String, String>) -> Result<UserId, ApiError> {
    path_params
        .get("partnerId")
        .map(UserId::try_from)
        .transpose()
        .map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_path_field("partnerId")
                .with_detail(msg)
        })?
        .ok_or(
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("partnerId")),
            )
            .with_path_field("partnerId")
            .with_detail("Missing field 'partnerId'."),
        )
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let partner_id = extract_partner_id_path(&event.payload.path_parameters)?;

    if user_id != partner_id {
        user_service.check_admin(&user_id).await?;
    }

    let shops: Vec<GetShopData> = get_shop_service
        .find_shops_by_partner(&partner_id)
        .await?
        .into_iter()
        .map(GetShopData::from)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(shops)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::shop_id::ShopId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use shop::core::shop::Shop;
    use shop::service::get_service::MockGetShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::{MockUserService, UserServiceError};

    #[tokio::test]
    async fn should_return_200_with_shops_when_requesting_own_partner_shops() {
        let user_id = UserId::new();
        let mut shop: Shop = Faker.fake();
        shop.shop_id = ShopId::new();

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_shops_by_partner()
            .return_once(move |_| Box::pin(async move { Ok(vec![shop]) }));

        let user_service = MockUserService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner/{partnerId}/shops")
                .path_parameter("partnerId", user_id.to_string())
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &get_shop_service, &user_service)
            .await
            .unwrap();
        assert_eq!(200, response.status_code);

        let body: serde_json::Value = match response.body {
            Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
                serde_json::from_str(&body_str).unwrap()
            }
            _ => panic!("Expected response body to be Text"),
        };
        assert!(body.as_array().unwrap().len() == 1);
    }

    #[tokio::test]
    async fn should_return_200_when_admin_requests_another_partners_shops() {
        let admin_user_id = UserId::new();
        let partner_user_id = UserId::new();

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_shops_by_partner()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner/{partnerId}/shops")
                .path_parameter("partnerId", partner_user_id.to_string())
                .jwt_claim("sub", admin_user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &get_shop_service, &user_service)
            .await
            .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_401_when_jwt_claim_missing() {
        let partner_user_id = UserId::new();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner/{partnerId}/shops")
                .path_parameter("partnerId", partner_user_id.to_string())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockUserService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_return_403_when_non_admin_requests_another_partners_shops() {
        let user_id = UserId::new();
        let partner_user_id = UserId::new();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner/{partnerId}/shops")
                .path_parameter("partnerId", partner_user_id.to_string())
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &MockGetShopService::default(), &user_service)
            .await
            .unwrap_err();
        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_return_400_when_partner_id_is_missing() {
        let user_id = UserId::new();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner/{partnerId}/shops")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockUserService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }
}
