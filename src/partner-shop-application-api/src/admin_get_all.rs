use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use partner_shop_application::data::get_partner_shop_application_data::GetPartnerShopApplicationData;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationService;
use user::service::user_service::UserService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl PartnerShopApplicationService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    user_service.check_admin(&user_id).await?;

    let applications: Vec<GetPartnerShopApplicationData> = service
        .find_all_partner_shop_applications()
        .await?
        .into_iter()
        .map(GetPartnerShopApplicationData::from)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(applications)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use partner_shop_application::{
        core::partner_shop_application::PartnerShopApplication,
        service::partner_shop_application_service::MockPartnerShopApplicationService,
    };
    use test_api::ApiGatewayV2httpRequestProxy;
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
    async fn should_200_with_empty_list_when_no_applications_exist() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_all_partner_shop_applications()
            .return_once(move || Box::pin(async move { Ok(vec![]) }));
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_with_applications_when_they_exist() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_all_partner_shop_applications()
            .return_once(move || {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(vec![app]) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_403_when_user_is_not_admin() {
        let user_id = UserId::new();
        let user_service = mock_non_admin_user_service();
        let service = MockPartnerShopApplicationService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/partner-applications")
                .jwt_claim("sub", user_id)
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
                .route_key("GET /api/v1/partner-applications")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(401, response.status);
    }
}
