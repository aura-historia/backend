use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use product_classification::category::data::get_category_summary_data::GetCategorySummaryData;
use product_classification::category::service::CategoryService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CategoryService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages: Vec<Language> = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect();

    let categories = service.view_categories(&languages).await?;

    let categories_data: Vec<GetCategorySummaryData> = categories
        .into_iter()
        .map(GetCategorySummaryData::from)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("public", Some(3600), Some(86400))
        .body_serde(categories_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use lambda_runtime::LambdaEvent;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_return_all_categories_for_get_all() {
        let mut service = MockCategoryService::default();
        service
            .expect_view_categories()
            .return_once(move |languages| {
                let categories: Vec<Category> = fake::vec![Category; 3];
                let localized = categories
                    .into_iter()
                    .map(|c| c.localized(languages))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_empty_list_when_no_categories_for_get_all() {
        let mut service = MockCategoryService::default();
        service
            .expect_view_categories()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
