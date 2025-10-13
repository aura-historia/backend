use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::ApiError;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_header;
use common::pagination::cursor::api::{TimeCursoredData, extract_time_cursor_query};
use common::user_id::api::extract_user_id_cognito_jwt;
use common::{
    api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
    sort::api::extract_sort_query,
};
use item_data::get_data::GetItemData;
use item_watchlist::sort_watch_item::{SortWatchlistItemField, SortWatchlistItemFieldData};
use item_watchlist::{domain::LocalizedWatchlistItemView, service::ItemWatchListService};
use lambda_runtime::LambdaEvent;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistItemDataView {
    pub item: GetItemData,

    pub notifications: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

impl From<LocalizedWatchlistItemView> for WatchlistItemDataView {
    fn from(view: LocalizedWatchlistItemView) -> Self {
        WatchlistItemDataView {
            item: view.item.into(),
            notifications: view.notifications,
            created: view.created,
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

// GET /api/v1/watchlist
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let language = extract_language_header(&event.payload.headers)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let sort =
        extract_sort_query::<SortWatchlistItemFieldData>(&event.payload.query_string_parameters)?
            .map(|sort_data| sort_data.map(SortWatchlistItemField::from));
    let cursor = extract_time_cursor_query(&event.payload.query_string_parameters)?;

    let items = service
        .view_watchlist(
            &user_id,
            &[language.into()],
            &currency.into(),
            &sort,
            &cursor,
        )
        .await?
        .map_item(WatchlistItemDataView::from);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(TimeCursoredData::from(items))?
        .cors()
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use item_watchlist::service::MockItemWatchListService;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockItemWatchListService::default();
        service
            .expect_view_watchlist()
            .return_once(|_, _, _, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .header("accept-language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("sort", "created")
                .query_string_parameter("order", "asc")
                .query_string_parameter("from", OffsetDateTime::now_utc().format(&Rfc3339).unwrap())
                .query_string_parameter("size", "10")
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_view_watchlist().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .header("accept-language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("from", OffsetDateTime::now_utc().format(&Rfc3339).unwrap())
                .query_string_parameter("size", "10")
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(401, response.status_code);
    }
}
