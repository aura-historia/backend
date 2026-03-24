use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::ApiError;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::pagination::cursor::api::{TimeCursoredData, extract_time_cursor_query};
use common::personalized::api::PersonalizedData;
use common::user_id::api::extract_user_id_request_context;
use common::{
    api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
    sort::api::extract_sort_query,
};
use lambda_runtime::LambdaEvent;
use product::data::get_data::GetProductData;
use product::data::user_state_data::{ProductUserStateData, WatchlistUserStateData};
use product_watchlist::core::watchlist_product::LocalizedWatchlistProductView;
use product_watchlist::data::sort_watchlist_product_field_data::SortWatchlistProductFieldData;
use product_watchlist::service::product_watchlist_service::ProductWatchListService;
use product_watchlist::service::sort_watchlist_product_field::SortWatchlistProductField;

fn to_personalized_data(
    view: LocalizedWatchlistProductView,
) -> PersonalizedData<GetProductData, ProductUserStateData> {
    PersonalizedData {
        item: GetProductData::from(view.product),
        user_state: Some(ProductUserStateData {
            watchlist: WatchlistUserStateData {
                watching: true,
                notifications: view.notifications,
            },
            ..Default::default()
        }),
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ProductWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let sort = extract_sort_query::<SortWatchlistProductFieldData>(
        &event.payload.query_string_parameters,
    )?
    .map(|sort_data| sort_data.map(SortWatchlistProductField::from));
    let cursor = extract_time_cursor_query(&event.payload.query_string_parameters)?;

    let products = service
        .view_watchlist(
            &user_id,
            &[language.into()],
            &currency.into(),
            &sort,
            &cursor,
        )
        .await?
        .map_item(to_personalized_data);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(TimeCursoredData::from(products))?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product_watchlist::service::product_watchlist_service::MockProductWatchListService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockProductWatchListService::default();
        service
            .expect_view_watchlist()
            .return_once(|_, _, _, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("sort", "created")
                .query_string_parameter("order", "asc")
                .query_string_parameter("from", OffsetDateTime::now_utc().format(&Rfc3339).unwrap())
                .query_string_parameter("size", "10")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockProductWatchListService::default();
        service.expect_view_watchlist().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("from", OffsetDateTime::now_utc().format(&Rfc3339).unwrap())
                .query_string_parameter("size", "10")
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_for_get_watchlist() {
        let mut service = MockProductWatchListService::default();
        service
            .expect_view_watchlist()
            .return_once(|_, _, _, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("sort", "created")
                .query_string_parameter("order", "asc")
                .query_string_parameter("from", OffsetDateTime::now_utc().format(&Rfc3339).unwrap())
                .query_string_parameter("size", "10")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "no-store",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
