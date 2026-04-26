use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use partner_shop_application::core::command::UpdatePartnerShopApplicationCommand;
use partner_shop_application::data::get_partner_shop_application_data::GetPartnerShopApplicationData;
use partner_shop_application::data::patch_partner_shop_application_data::PatchPartnerShopApplicationData;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationService;

use crate::path::extract_partner_application_id_path;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PartnerShopApplicationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let application_id = extract_partner_application_id_path(&event.payload.path_parameters)?;

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let patch_data: PatchPartnerShopApplicationData =
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?;

    let update_cmd = UpdatePartnerShopApplicationCommand {
        shop_name: patch_data.shop_name,
        shop_type: patch_data.shop_type.map(Into::into),
        shop_domains: patch_data.shop_domains,
        shop_image: patch_data.shop_image,
        shop_structured_address: patch_data.shop_structured_address.map(Into::into),
        shop_phone: patch_data.shop_phone,
        shop_email: patch_data.shop_email,
        shop_specialities_categories: patch_data.shop_specialities_categories,
        shop_specialities_periods: patch_data.shop_specialities_periods,
    };

    let data: GetPartnerShopApplicationData = service
        .update_partner_shop_application(&user_id, &application_id, update_cmd)
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(data.updated)
        .body_serde(data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use partner_shop_application::{
        core::{
            partner_shop_application::PartnerShopApplication,
            partner_shop_application_id::PartnerShopApplicationId,
        },
        data::patch_partner_shop_application_data::PatchPartnerShopApplicationData,
        service::partner_shop_application_service::{
            MockPartnerShopApplicationService, PartnerShopApplicationError,
        },
    };
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::MockUserService;

    #[tokio::test]
    async fn should_200_when_updating_application() {
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_update_partner_shop_application()
            .return_once(move |_, _, _| {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(app) })
            });
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", UserId::new())
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&Faker.fake::<PatchPartnerShopApplicationData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_jwt_claim_sub_is_missing() {
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/partner-applications/{partnerApplicationId}")
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&Faker.fake::<PatchPartnerShopApplicationData>())
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
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", UserId::new())
                .body_serde(&Faker.fake::<PatchPartnerShopApplicationData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_empty() {
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", UserId::new())
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
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
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_update_partner_shop_application()
            .return_once(move |user_id, id, _| {
                let user_id = *user_id;
                let id = *id;
                Box::pin(async move { Err(PartnerShopApplicationError::NotFound(user_id, id)) })
            });
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/partner-applications/{partnerApplicationId}")
                .jwt_claim("sub", UserId::new())
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&Faker.fake::<PatchPartnerShopApplicationData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(404, response.status);
    }
}
