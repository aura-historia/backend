use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use product_classification::category::data::category_search::{CategorySearch, CategorySearchData};
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

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty. If you want to search without any restrictions, supply the body '{}'.";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;

    let search_data: CategorySearchData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let search: CategorySearch = search_data.into();

    let categories = if search.is_empty() {
        service
            .view_categories(&languages)
            .await
            .map_err(crate::category_service_error_to_api_error)?
    } else {
        service
            .search_categories(&search, &languages)
            .await
            .map_err(crate::category_service_error_to_api_error)?
    };

    let categories_data: Vec<GetCategorySummaryData> = categories
        .into_iter()
        .map(GetCategorySummaryData::from)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(categories_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use lambda_runtime::LambdaEvent;
    use product_classification::category::core::Category;
    use product_classification::category::data::category_search::CategorySearchData;
    use product_classification::category::service::MockCategoryService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_search_categories_when_name_query_present_for_search() {
        let mut service = MockCategoryService::default();
        service
            .expect_search_categories()
            .return_once(move |_, languages| {
                let categories: Vec<Category> = fake::vec![Category; 2];
                let localized = categories
                    .into_iter()
                    .map(|c| c.localized(languages))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let search_data = CategorySearchData {
            name_query: Some("Furniture".try_into().unwrap()),
        };
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/categories/search")
                .body_serde(&search_data)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_use_find_categories_when_empty_search_for_search() {
        let mut service = MockCategoryService::default();
        service.expect_search_categories().never();
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
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/categories/search")
                .body_serde(&CategorySearchData::default())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_is_missing_for_search() {
        let mut service = MockCategoryService::default();
        service.expect_search_categories().never();
        service.expect_view_categories().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/categories/search")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_invalid_json_for_search() {
        let mut service = MockCategoryService::default();
        service.expect_search_categories().never();
        service.expect_view_categories().never();
        let mut payload: aws_lambda_events::apigw::ApiGatewayV2httpRequest =
            ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/categories/search")
                .build();
        payload.body = Some("invalid-json".to_string());
        let lambda_event = LambdaEvent {
            payload,
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(400, response.status);
    }
}
