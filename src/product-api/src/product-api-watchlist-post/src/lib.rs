use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::BAD_BODY_VALUE;
use common::product_id::api::ProductKeyData;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use product::watchlist::data::watchlist_product_data::WatchlistProductData;
use product::watchlist::service::product_watchlist_service::ProductWatchListService;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ProductWatchListService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// POST /api/v1/me/watchlist
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ProductWatchListService,
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
    let product_key_data: ProductKeyData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let watchlist_product = service
        .create_watchlist_product(
            &user_id,
            &product_key_data.shop_id,
            &product_key_data.shops_product_id,
        )
        .await?;

    let location = match event.payload.request_context.domain_name {
        None => None,
        Some(domain_name) => match event.payload.request_context.stage {
            Some(stage_name) => Some(format!(
                "https://{domain_name}/{stage_name}/api/v1/me/watchlist/{}/{}",
                product_key_data.shop_id, product_key_data.shops_product_id
            )),
            None => None,
        },
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .try_location(location.as_deref())
        .body_serde(WatchlistProductData::from(watchlist_product))?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::{product_id::api::ProductKeyData, user_id::UserId};
    use fake::{Fake, Faker};
    use http::header::LOCATION;
    use lambda_runtime::LambdaEvent;
    use product::watchlist::service::product_watchlist_service::MockProductWatchListService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_201_when_success() {
        let mut service = MockProductWatchListService::default();
        service
            .expect_create_watchlist_product()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let product_key_data = Faker.fake::<ProductKeyData>();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&product_key_data)
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
                product_key_data.shop_id, product_key_data.shops_product_id
            ),
            response.headers.get(LOCATION).unwrap().to_str().unwrap()
        )
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockProductWatchListService::default();
        service.expect_create_watchlist_product().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&Faker.fake::<ProductKeyData>())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(401, response.status_code);
    }
}
