use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use user::data::get_user_data::GetUserAccountData;
use user::data::patch_user_data::PatchUserAccountData;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let patch_user_account_data: PatchUserAccountData =
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?;

    let update_user_command = UpdateUserCommand {
        first_name: patch_user_account_data.first_name,
        last_name: patch_user_account_data.last_name,
        language: patch_user_account_data.language.map(Language::from),
        currency: patch_user_account_data.currency.map(Currency::from),
        prohibited_content_consent: patch_user_account_data.prohibited_content_consent,
        tier: None,
        role: None,
    };
    let updated_user_account_data: GetUserAccountData = service
        .update_user(&user_id, update_user_command)
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(updated_user_account_data.updated)
        .body_serde(updated_user_account_data)?
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
        data::patch_user_data::PatchUserAccountData,
        service::user_service::{MockUserService, UserServiceError},
    };

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockUserService::default();
        service.expect_update_user().return_once(move |_, _| {
            let mut user: User = Faker.fake();
            user.updated = timestamp;
            Box::pin(async move { Ok(user) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/account")
                .body_serde(&Faker.fake::<PatchUserAccountData>())
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
        let service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/account")
                .body_serde(&Faker.fake::<PatchUserAccountData>())
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
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/account")
                .jwt_claim("sub", UserId::new())
                .body_serde(&Faker.fake::<PatchUserAccountData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserService::default();
        service.expect_update_user().return_once(move |user_id, _| {
            let user_id = *user_id;
            Box::pin(async move { Err(UserServiceError::UserNotFound(user_id)) })
        });

        let response = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(404, response.status);
    }
}
