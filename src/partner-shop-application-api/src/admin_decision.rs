use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use partner_shop_application::data::decision_data::PostPartnerShopApplicationDecisionData;
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

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let post_data: PostPartnerShopApplicationDecisionData =
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?;

    let data: GetPartnerShopApplicationData = service
        .submit_decision_by_id(&application_id, post_data.decision.into())
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
        data::decision_data::{
            PartnerShopApplicationDecisionData, PostPartnerShopApplicationDecisionData,
        },
        service::partner_shop_application_service::{
            MockPartnerShopApplicationService, PartnerShopApplicationError,
        },
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
    async fn should_200_when_submitting_approve_decision() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_submit_decision_by_id()
            .return_once(move |_, _| {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(app) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/partner-applications/{partnerApplicationId}/decision")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&PostPartnerShopApplicationDecisionData {
                    decision: PartnerShopApplicationDecisionData::Approve,
                })
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_when_submitting_reject_decision() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_submit_decision_by_id()
            .return_once(move |_, _| {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(app) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/partner-applications/{partnerApplicationId}/decision")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&PostPartnerShopApplicationDecisionData {
                    decision: PartnerShopApplicationDecisionData::Reject,
                })
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
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/partner-applications/{partnerApplicationId}/decision")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&PostPartnerShopApplicationDecisionData {
                    decision: PartnerShopApplicationDecisionData::Approve,
                })
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_empty() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let service = MockPartnerShopApplicationService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/partner-applications/{partnerApplicationId}/decision")
                .jwt_claim("sub", user_id)
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
    async fn should_409_when_application_not_in_review() {
        let user_id = UserId::new();
        let user_service = mock_admin_user_service();
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_submit_decision_by_id()
            .return_once(move |id, _| {
                let id = *id;
                Box::pin(async move { Err(PartnerShopApplicationError::NotInReviewState(id)) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/partner-applications/{partnerApplicationId}/decision")
                .jwt_claim("sub", user_id)
                .path_parameter("partnerApplicationId", PartnerShopApplicationId::new())
                .body_serde(&PostPartnerShopApplicationDecisionData {
                    decision: PartnerShopApplicationDecisionData::Approve,
                })
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(409, response.status);
    }
}
