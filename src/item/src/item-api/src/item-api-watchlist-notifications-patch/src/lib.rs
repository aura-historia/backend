use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{
    BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, INVALID_RFC3339_TIMESTAMP,
};
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shop_id::api::extract_shop_id_path;
use common::shops_item_id::ShopsItemId;
use common::shops_item_id::api::extract_shops_item_id_path;
use common::user_id::api::extract_user_id_cognito_jwt;
use item_watchlist::domain::WatchlistItem;
use item_watchlist::service::ItemWatchListService;
use lambda_runtime::LambdaEvent;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistItemPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistItemData {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub item_id: ItemId,
    pub notifications: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<WatchlistItem> for WatchlistItemData {
    fn from(domain: WatchlistItem) -> Self {
        WatchlistItemData {
            shop_id: domain.shop_id,
            shops_item_id: domain.shops_item_id,
            item_id: domain.item_id,
            notifications: domain.notifications,
            created: domain.created,
            updated: domain.updated,
        }
    }
}

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// PATCH /api/v1/watchlist/{shopId}/{shopsItemId}?created={timestamp}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let _ = extract_shop_id_path(&event.payload.path_parameters)?;
    let _ = extract_shops_item_id_path(&event.payload.path_parameters)?;
    let created = event
        .payload
        .query_string_parameters
        .first("created")
        .filter(|val| !val.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("created")
                .with_message("The timestamp of when the watchlist-entry was created is required to delete it.")
        })
        .map(|val| OffsetDateTime::parse(val, &Rfc3339))?
        .map_err(|err| ApiError::bad_request(INVALID_RFC3339_TIMESTAMP).with_query_field("created").with_message(err.to_string()))?;
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE).with_message("Body cannot be empty")
        })?;
    let patch: WatchlistItemPatch = serde_json::from_str(&body)
        .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string()))?;

    let watchlist_item = match patch.notifications {
        Some(notfications) => {
            service
                .toggle_notifications(&user_id, &created, &notfications)
                .await?
        }
        None => service.find_watchlist_item(&user_id, &created).await?,
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cors()
        .body_serde(WatchlistItemData::from(watchlist_item))?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::{WatchlistItemPatch, handler};
    use common::{shop_id::ShopId, shops_item_id::ShopsItemId, user_id::UserId};
    use fake::{Fake, Faker};
    use item_watchlist::service::MockItemWatchListService;
    use lambda_runtime::LambdaEvent;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockItemWatchListService::default();
        service
            .expect_toggle_notifications()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistItemPatch {
                    notifications: Some(true),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_shop_id_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistItemPatch {
                    notifications: Faker.fake(),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_PATH_PARAMETER_VALUE", json["error"]);
        assert_eq!("shopId", json["source"]["field"]);
        assert_eq!("PATH", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_400_when_shops_item_id_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistItemPatch {
                    notifications: Faker.fake(),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_PATH_PARAMETER_VALUE", json["error"]);
        assert_eq!("shopsItemId", json["source"]["field"]);
        assert_eq!("PATH", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_400_when_created_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .body_serde(&WatchlistItemPatch {
                    notifications: Faker.fake(),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_QUERY_PARAMETER_VALUE", json["error"]);
        assert_eq!("created", json["source"]["field"]);
        assert_eq!("QUERY", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_400_when_created_invalid() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter("created", "boooop")
                .body_serde(&WatchlistItemPatch {
                    notifications: Faker.fake(),
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("INVALID_RFC3339_TIMESTAMP", json["error"]);
        assert_eq!("created", json["source"]["field"]);
        assert_eq!("QUERY", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_400_when_body_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_BODY_VALUE", json["error"]);
    }

    #[tokio::test]
    async fn should_400_when_body_invalid() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&"foo")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_BODY_VALUE", json["error"]);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .body_serde(&WatchlistItemPatch {
                    notifications: Faker.fake(),
                })
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(401, response.status_code);
    }
}
