use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::shop_id::api::extract_shop_id_path;
use common::shops_product_id::api::extract_shops_product_id_path;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use product_watchlist::{
    data::watchlist_product_data::WatchlistProductData,
    service::{
        command::UpdateWatchlistProductCommand, product_watchlist_service::ProductWatchListService,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistProductPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
}

impl From<WatchlistProductPatch> for UpdateWatchlistProductCommand {
    fn from(patch: WatchlistProductPatch) -> Self {
        UpdateWatchlistProductCommand {
            notifications: patch.notifications,
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ProductWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
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
    let patch: WatchlistProductPatch = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let watchlist_product = service
        .update_watchlist_product(&user_id, &shop_id, &shops_product_id, patch.into())
        .await?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(watchlist_product.updated)
        .body_serde(WatchlistProductData::from(watchlist_product))?
        .build())
}

#[cfg(test)]
mod tests {
    use super::WatchlistProductPatch;
    use super::handle;
    use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use product_watchlist::service::product_watchlist_service::MockProductWatchListService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockProductWatchListService::default();
        service
            .expect_update_watchlist_product()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistProductPatch {
                    notifications: Some(true),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_shop_id_missing() {
        let mut service = MockProductWatchListService::default();
        service.expect_delete_watchlist_product().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopsProductId", ShopsProductId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistProductPatch {
                    notifications: Faker.fake(),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_shops_product_id_missing() {
        let mut service = MockProductWatchListService::default();
        service.expect_delete_watchlist_product().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistProductPatch {
                    notifications: Faker.fake(),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_body_missing() {
        let mut service = MockProductWatchListService::default();
        service.expect_delete_watchlist_product().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_body_invalid() {
        let mut service = MockProductWatchListService::default();
        service.expect_delete_watchlist_product().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&"foo")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockProductWatchListService::default();
        service.expect_delete_watchlist_product().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistProductPatch {
                    notifications: Faker.fake(),
                })
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }
}
