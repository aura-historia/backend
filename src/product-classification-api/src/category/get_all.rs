use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::language::data::api::extract_language_query;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::query::any_of_query::AnyOfQuery;
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::query_service::QueryProductService;
use product_classification::category::data::get_category_summary_data::GetCategorySummaryData;
use product_classification::category::service::CategoryService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CategoryService,
    query_product_service: &impl QueryProductService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages = vec![Language::from(extract_language_query(
        &event.payload.query_string_parameters,
    )?)];

    let categories = service.view_categories(&languages).await?;

    let mut categories_data = Vec::with_capacity(categories.len());
    for category in categories {
        let product_search = ProductSearch::default()
            .with_category_id(AnyOfQuery::from_iter([category.category_id.clone()]));
        let cursor = Cursor {
            search_after: None,
            size: 0,
        };
        let products = query_product_service
            .search_products(&product_search, &None, &Some(cursor))
            .await?;
        let category_data = GetCategorySummaryData::from_category_with_product_count(
            category,
            products.total.unwrap_or(0) as u32,
        );
        categories_data.push(category_data);
    }

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("public", Some(600), Some(3600))
        .body_serde(categories_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::pagination::cursor::{Cursor, CursoredResult};
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::service::query_service::MockQueryProductService;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::service::MockPeriodService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_return_all_categories_for_get_all() {
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_view_categories()
            .return_once(move |languages| {
                let categories: Vec<Category> = fake::vec![Category; 3];
                let localized = categories
                    .into_iter()
                    .map(|c| c.localized(languages))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let period_service = MockPeriodService::default();
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products()
            .times(3)
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: Vec::<LocalizedProductView>::new(),
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: Some(0),
                    })
                })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &category_service,
            &period_service,
            &query_product_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_empty_list_when_no_categories_for_get_all() {
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_view_categories()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &category_service,
            &period_service,
            &query_product_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_with_long_max_ages_for_get_all_categories() {
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_view_categories()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &category_service,
            &period_service,
            &query_product_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=600, s-maxage=3600",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
