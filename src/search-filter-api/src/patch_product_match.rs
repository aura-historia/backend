use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    actor::{RequestContext, domain::Actor},
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    shop_id::api::extract_shop_id_path,
    shops_product_id::api::extract_shops_product_id_path,
    user_id::api::extract_user_id_request_context,
    user_search_filter_id::api::extract_user_search_filter_id_path,
};
use lambda_runtime::LambdaEvent;
use search_filter::core::command::UpdateUserSearchFilterMatchCommand;
use search_filter::data::search_filter_product_match_data::SearchFilterProductMatchData;
use search_filter::service::user_search_filter_service::UserSearchFilterService;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserSearchFilterMatchData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<bool>,
}

impl From<PatchUserSearchFilterMatchData> for UpdateUserSearchFilterMatchCommand {
    fn from(patch: PatchUserSearchFilterMatchData) -> Self {
        UpdateUserSearchFilterMatchCommand {
            feedback: patch.feedback,
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = extract_user_search_filter_id_path(&event.payload.path_parameters)?;
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_product_id = extract_shops_product_id_path(&event.payload.path_parameters)?;
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let patch: PatchUserSearchFilterMatchData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let patched: SearchFilterProductMatchData = service
        .update_search_filter_product_match(
            &RequestContext {
                actor: Actor::User(user_id),
            },
            user_id,
            search_filter_id,
            shop_id,
            shops_product_id,
            patch.into(),
        )
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(patched.updated)
        .body_serde(patched)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::{PatchUserSearchFilterMatchData, handle};
    use common::{
        shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId,
        user_search_filter_id::UserSearchFilterId,
    };
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use search_filter::service::user_search_filter_service::{
        MockUserSearchFilterService, UserSearchFilterError,
    };
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_update_search_filter_product_match()
            .return_once(|_, _, _, _, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .body_serde(&PatchUserSearchFilterMatchData {
                    feedback: Some(true),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_missing() {
        let mut service = MockUserSearchFilterService::default();
        service.expect_update_search_filter_product_match().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_shop_id_missing() {
        let mut service = MockUserSearchFilterService::default();
        service.expect_update_search_filter_product_match().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .body_serde(&PatchUserSearchFilterMatchData {
                    feedback: Some(false),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_404_when_match_not_found() {
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_update_search_filter_product_match()
            .return_once(|_, _, _, _, _, _| {
                Box::pin(async {
                    Err(UserSearchFilterError::UserSearchFilterMatchNotFound(
                        Faker.fake(),
                        Faker.fake(),
                        Faker.fake(),
                        Faker.fake(),
                    ))
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .body_serde(&PatchUserSearchFilterMatchData {
                    feedback: Some(false),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(404, actual.status);
    }
}
