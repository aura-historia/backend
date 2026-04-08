use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use partner_shop_application::data::get_partner_shop_application_data::GetPartnerShopApplicationData;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PartnerShopApplicationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let applications: Vec<GetPartnerShopApplicationData> = service
        .find_all_partner_shop_applications_by_user(&user_id)
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

    #[tokio::test]
    async fn should_200_with_empty_list_when_no_applications_exist() {
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_all_partner_shop_applications_by_user()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-applications")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_with_applications_when_they_exist() {
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_find_all_partner_shop_applications_by_user()
            .return_once(move |_| {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(vec![app]) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-applications")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_jwt_claim_sub_is_missing() {
        let service = MockPartnerShopApplicationService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-applications")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, response.status);
    }
}
