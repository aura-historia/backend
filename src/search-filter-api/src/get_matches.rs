use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::ApiError;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::pagination::cursor::api::{TimeCursoredData, extract_time_cursor_query};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::personalized::api::PersonalizedData;
use common::product_id::ProductKey;
use common::user_id::api::extract_user_id_request_context;
use common::user_search_filter_id::api::extract_user_search_filter_id_path;
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
use search_filter::core::sort_search_filter_match_field::SortSearchFilterMatchField;
use search_filter::data::sort_search_filter_match_field_data::SortSearchFilterMatchFieldData;
use search_filter::service::user_search_filter_service::UserSearchFilterService;
use std::collections::HashMap;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
    get_product_service: &(impl GetProductService + Sync),
    personalization_service: &(impl ProductPersonalizationService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = extract_user_search_filter_id_path(&event.payload.path_parameters)?;
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let sort = extract_sort_query::<SortSearchFilterMatchFieldData>(
        &event.payload.query_string_parameters,
    )?
    .map(|sort_data| sort_data.map(SortSearchFilterMatchField::from));
    let cursor = extract_time_cursor_query(&event.payload.query_string_parameters)?;

    // 1. Get paged search filter match entries (sorted by created timestamp)
    let matches_page = service
        .view_search_filter_matches(&user_id, &search_filter_id, &sort, cursor)
        .await?;

    // 2. Fetch localized products for current page (batch, unordered)
    let product_keys: Vec<ProductKey> = matches_page
        .items
        .iter()
        .map(|m| ProductKey::new(m.shop_id, m.shops_product_id.clone()))
        .collect();
    let localized_products = get_product_service
        .view_products(product_keys, &[language.into()], &currency.into())
        .await?;

    // 3. Map by product_id to restore match sort order (batch-get returns in arbitrary order)
    let mut product_map: HashMap<_, LocalizedProductView> = localized_products
        .into_iter()
        .map(|p| (p.product_id, p))
        .collect();
    let ordered_localized: Vec<LocalizedProductView> = matches_page
        .items
        .iter()
        .filter_map(|m| {
            if let Some(p) = product_map.remove(&m.product_id) {
                Some(p)
            } else {
                tracing::warn!(
                    productId = %m.product_id,
                    "Could not find product for search filter match entry. Skipping."
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
                .clone()
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
            search_after: matches_page.cursor.search_after,
        },
        items,
        total: matches_page.total,
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(TimeCursoredData::from(result))?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::{user_id::UserId, user_search_filter_id::UserSearchFilterId};
    use fake::{Fake, Faker};
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product::service::get_service::MockGetProductService;
    use product_personalization::service::MockProductPersonalizationService;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_view_search_filter_matches()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Faker.fake()) }));
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
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
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
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockUserSearchFilterService::default();
        service.expect_view_search_filter_matches().never();
        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store() {
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_view_search_filter_matches()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Faker.fake()) }));
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
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
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
            &service,
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
