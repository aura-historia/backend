use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use partner_shop_application::data::get_partner_shop_application_data::GetPartnerShopApplicationData;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationService;
use user::service::user_service::UserService;

use crate::path::extract_partner_application_id_path;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl PartnerShopApplicationService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    user_service.check_admin(&user_id).await?;

    let application_id = extract_partner_application_id_path(&event.payload.path_parameters)?;

    let data: GetPartnerShopApplicationData = service
        .find_partner_shop_application_by_id(&application_id)
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(data.updated)
        .cache_control("no-store", None, None)
        .body_serde(data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use http::header::LAST_MODIFIED;
    use lambda_runtime::LambdaEvent;
    use partner_shop_application::{
        core::{
            partner_shop_application::PartnerShopApplication,
            partner_shop_application_id::PartnerShopApplicationId,
        },
        service::partner_shop_application_service::{
            MockPartnerShopApplicationService, PartnerShopApplicationError,
        },
    };
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;
    use user::service::user_service::{MockUserService, UserServiceError};

    fn mock_admin_user_service() -> MockUserService {
        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async move { Ok(()) }));
        user_service
    }

    fn mock_non_admin_user_service() -> MockUserService {
        let mut user_service = MockUserService::default();
        user_service.expect_check_admin().return_once(move |_| {
            Box::pin(async move { Err(UserServiceError::AdminRoleRequired) })
        });
        user_service
    }

    #[tokio::test]
    async fn should_200_when_application_exists() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_partner_shop_application_by_id()
            .return_once(move |_| {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(app) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_partner_shop_application_by_id()
            .return_once(move |_| {
                let mut app: PartnerShopApplication = Faker.fake();
                app.updated = timestamp;
                Box::pin(async move { Ok(app) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_403_when_user_is_not_admin() {
        let user_id = UserId::new();
        let user_service = mock_non_admin_user_service();
        let service = MockPartnerShopApplicationService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_401_when_jwt_claim_sub_is_missing() {
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_400_when_path_param_partner_application_id_missing() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let service = MockPartnerShopApplicationService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_partner_application_id_invalid() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let service = MockPartnerShopApplicationService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", "not-a-valid-uuid")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_404_when_application_not_exists() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_partner_shop_application_by_id()
            .return_once(move |id| {
                let id = *id;
                Box::pin(async move { Err(PartnerShopApplicationError::NotFoundById(id)) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(404, response.status);
    }
}
