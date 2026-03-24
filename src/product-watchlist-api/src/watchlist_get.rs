use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::ApiError;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::pagination::cursor::api::{TimeCursoredData, extract_time_cursor_query};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::personalized::api::PersonalizedData;
use common::product_id::ProductKey;
use common::user_id::api::extract_user_id_request_context;
use common::{
    api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
    sort::api::extract_sort_query,
};
use lambda_runtime::LambdaEvent;
use product::core::product::LocalizedProductView;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::service::get_service::GetProductService;
use product_personalization::service::ProductPersonalizationService;
use product_watchlist::data::sort_watchlist_product_field_data::SortWatchlistProductFieldData;
use product_watchlist::service::product_watchlist_service::ProductWatchListService;
use product_watchlist::service::sort_watchlist_product_field::SortWatchlistProductField;
use std::collections::HashMap;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    watchlist_service: &impl ProductWatchListService,
    get_product_service: &impl GetProductService,
    personalization_service: &impl ProductPersonalizationService,
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

    // 1. Get paged watchlist entries (sorted by created timestamp)
    let watchlist_page = watchlist_service
        .view_watchlist(&user_id, &sort, &cursor)
        .await?;

    // 2. Fetch localized products for current page (batch, unordered)
    let product_keys: Vec<ProductKey> = watchlist_page
        .items
        .iter()
        .map(|wp| ProductKey::new(wp.shop_id, wp.shops_product_id.clone()))
        .collect();
    let localized_products = get_product_service
        .view_products(product_keys, &[language.into()], &currency.into())
        .await?;

    // 3. Map by product_id to restore watchlist sort order (batch-get returns in arbitrary order)
    let mut product_map: HashMap<_, LocalizedProductView> = localized_products
        .into_iter()
        .map(|p| (p.product_id, p))
        .collect();
    let ordered_localized: Vec<LocalizedProductView> = watchlist_page
        .items
        .iter()
        .filter_map(|wp| {
            if let Some(p) = product_map.remove(&wp.product_id) {
                Some(p)
            } else {
                tracing::error!(
                    productId = %wp.product_id,
                    "Could not find product for watchlist entry. Skipping."
                );
                None
            }
        })
        .collect();

    // 4. Personalize all products for the authenticated user
    let personalized = personalization_service
        .personalize_all(&user_id, ordered_localized)
        .await?;

    // 5. Map to PersonalizedData and rebuild cursor with final item count
    let items: Vec<PersonalizedData<GetProductData, ProductUserStateData>> = personalized
        .into_iter()
        .map(|p| {
            let consent = p
                .user_state
                .map(|s| s.prohibited_content.consent)
                .unwrap_or(false);
            PersonalizedData {
                item: GetProductData::from_view(p.item, consent),
                user_state: p.user_state.map(ProductUserStateData::from),
            }
        })
        .collect();

    let result = CursoredResult {
        cursor: Cursor {
            size: items.len() as u64,
            search_after: watchlist_page.cursor.search_after,
        },
        items,
        total: watchlist_page.total,
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(TimeCursoredData::from(result))?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product::service::get_service::MockGetProductService;
    use product_personalization::service::MockProductPersonalizationService;
    use product_watchlist::service::product_watchlist_service::MockProductWatchListService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_200_when_success() {
        let mut watchlist_service = MockProductWatchListService::default();
        watchlist_service
            .expect_view_watchlist()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));
        let mut get_product_service = MockGetProductService::default();
        get_product_service
            .expect_view_products()
            .return_once(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

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

        let response = handle(
            lambda_event,
            &watchlist_service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut watchlist_service = MockProductWatchListService::default();
        watchlist_service.expect_view_watchlist().never();
        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();

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

        let actual = handle(
            lambda_event,
            &watchlist_service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_for_get_watchlist() {
        let mut watchlist_service = MockProductWatchListService::default();
        watchlist_service
            .expect_view_watchlist()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));
        let mut get_product_service = MockGetProductService::default();
        get_product_service
            .expect_view_products()
            .return_once(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

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

        let response = handle(
            lambda_event,
            &watchlist_service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();

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
