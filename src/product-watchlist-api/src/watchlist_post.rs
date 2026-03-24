use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::personalized::api::PersonalizedData;
use common::product_id::api::ProductKeyData;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use product::data::get_data::GetProductData;
use product::data::user_state_data::{ProductUserStateData, WatchlistUserStateData};
use product_watchlist::service::product_watchlist_service::ProductWatchListService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ProductWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
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
            &[language.into()],
            &currency.into(),
        )
        .await?;

    let personalized = PersonalizedData {
        item: GetProductData::from(watchlist_product.product),
        user_state: Some(ProductUserStateData {
            watchlist: WatchlistUserStateData {
                watching: true,
                notifications: watchlist_product.notifications,
            },
            ..Default::default()
        }),
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .location(
            &format!(
                "me/watchlist/{}/{}",
                product_key_data.shop_id, product_key_data.shops_product_id
            ),
            &event.payload.request_context,
        )
        .body_serde(personalized)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::{product_id::api::ProductKeyData, user_id::UserId};
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use product_watchlist::service::product_watchlist_service::MockProductWatchListService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_201_when_success() {
        let mut service = MockProductWatchListService::default();
        service
            .expect_create_watchlist_product()
            .return_once(|_, _, _, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let product_key_data = Faker.fake::<ProductKeyData>();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&product_key_data)
                .jwt_claim("sub", UserId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(201, response.status_code);
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

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }
}
