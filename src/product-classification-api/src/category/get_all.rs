use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::language::data::api::extract_language_query;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use product_classification::category::data::get_category_summary_data::GetCategorySummaryData;
use product_classification::category::service::CategoryService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CategoryService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages = vec![Language::from(extract_language_query(
        &event.payload.query_string_parameters,
    )?)];

    let categories = service.view_categories(&languages).await?;

    let categories_data: Vec<GetCategorySummaryData> = categories
        .into_iter()
        .map(GetCategorySummaryData::from)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("public", Some(86400), Some(604800))
        .body_serde(categories_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
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
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
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
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
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
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=86400, s-maxage=604800",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
