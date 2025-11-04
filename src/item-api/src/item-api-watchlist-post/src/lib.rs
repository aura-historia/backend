use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::item_id::api::ItemKeyData;
use common::user_id::api::extract_user_id_cognito_jwt;
use item::watchlist::data::watchlist_item_data::WatchlistItemData;
use item::watchlist::service::item_watchlist_service::ItemWatchListService;
use lambda_runtime::LambdaEvent;

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

// POST /api/v1/me/watchlist
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE).with_message("Body cannot be empty")
        })?;
    let item_key_data: ItemKeyData = serde_json::from_str(&body)
        .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string()))?;

    let watchlist_item = service
        .create_watchlist_item(
            &user_id,
            &item_key_data.shop_id,
            &item_key_data.shops_item_id,
        )
        .await?;

    let location = match event.payload.request_context.domain_name {
        None => None,
        Some(domain_name) => match event.payload.request_context.stage {
            Some(stage_name) => Some(format!(
                "https://{domain_name}/{stage_name}/api/v1/me/watchlist/{}/{}",
                item_key_data.shop_id, item_key_data.shops_item_id
            )),
            None => None,
        },
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .try_location(location.as_deref())
        .body_serde(WatchlistItemData::from(watchlist_item))?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::{item_id::api::ItemKeyData, user_id::UserId};
    use fake::{Fake, Faker};
    use http::header::LOCATION;
    use item::watchlist::service::item_watchlist_service::MockItemWatchListService;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_201_when_success() {
        let mut service = MockItemWatchListService::default();
        service
            .expect_create_watchlist_item()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let item_key_data = Faker.fake::<ItemKeyData>();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&item_key_data)
                .jwt_claim("sub", UserId::new())
                .domain_name("my.domain.com")
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(201, response.status_code);
        assert_eq!(
            format!(
                "https://my.domain.com/prod/api/v1/me/watchlist/{}/{}",
                item_key_data.shop_id, item_key_data.shops_item_id
            ),
            response.headers.get(LOCATION).unwrap().to_str().unwrap()
        )
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_create_watchlist_item().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&Faker.fake::<ItemKeyData>())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(401, response.status_code);
    }
}
