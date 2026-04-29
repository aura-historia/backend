use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::pagination::cursor::Cursor;
use common::query::any_of_query::AnyOfQuery;
use common::sort::api::extract_sort_query;
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::query_service::QueryProductService;
use product_classification::category::category_search::CategorySearchData;
use product_classification::category::data::get_category_summary_data::GetCategorySummaryData;
use product_classification::category::data::sort_category_field_data::SortCategoryFieldData;
use product_classification::category::service::CategoryService;
use product_classification::category::sort_category_field::SortCategoryField;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CategoryService,
    query_product_service: &impl QueryProductService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let sort = extract_sort_query::<SortCategoryFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortCategoryField::from));

    let search_data: CategorySearchData = if event.payload.route_key.as_deref()
        == Some("GET /api/v1/categories")
    {
        let query = event
            .payload
            .raw_query_string
            .clone()
            .filter(|query| !query.is_empty())
            .unwrap_or_else(|| {
                let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                for (key, value) in event.payload.query_string_parameters.iter() {
                    serializer.append_pair(key, value);
                }
                serializer.finish()
            });
        serde_qs::from_str(&query).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?
    } else {
        let body = event
            .payload
            .body
            .filter(|str| !str.is_empty())
            .ok_or_else(|| {
                let err_msg = "Body cannot be empty. If you want to search without any restrictions, supply the body '{}'.";
                ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
            })?;

        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?
    };

    let categories = service
        .search_categories(&search_data.into(), &sort)
        .await?;

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

    let response_builder = ApiGatewayV2HttpResponseBuilder::json(200);
    let response_builder = if event.payload.route_key.as_deref() == Some("GET /api/v1/categories") {
        response_builder.cache_control("public", Some(600), Some(3600))
    } else {
        response_builder
    };

    Ok(response_builder.body_serde(categories_data)?.build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::pagination::cursor::{Cursor, CursoredResult};
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::service::query_service::MockQueryProductService;
    use product_classification::category::category_search::CategorySearchData;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::service::MockPeriodService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_search_categories_when_name_query_present_for_search() {
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_search_categories()
            .return_once(move |search, _| {
                let categories: Vec<Category> = fake::vec![Category; 2];
                let localized = categories
                    .into_iter()
                    .map(|c| c.localized(&[search.language]))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let period_service = MockPeriodService::default();
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products()
            .times(2)
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
        let search_data = CategorySearchData {
            language: common::language::data::LanguageData::Es,
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
    async fn should_search_categories_when_get_query_params_for_search() {
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_search_categories()
            .return_once(move |search, _| {
                assert_eq!(common::language::domain::Language::Es, search.language);
                assert_eq!(Some("Furniture".try_into().unwrap()), search.name_query);
                Box::pin(async move { Ok(vec![]) })
            });
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .query_string_parameter("language", "es")
                .query_string_parameter("nameQuery", "Furniture")
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
                .get(http::header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_search_categories_when_empty_search_for_search() {
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_search_categories()
            .return_once(move |search, _| {
                let categories: Vec<Category> = fake::vec![Category; 3];
                let localized = categories
                    .into_iter()
                    .map(|c| c.localized(&[search.language]))
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
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/categories/search")
                .body_serde(&CategorySearchData::default())
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
    async fn should_400_when_body_is_missing_for_search() {
        let mut category_service = MockCategoryService::default();
        category_service.expect_search_categories().never();
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/categories/search")
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
        .unwrap_err();

        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_invalid_json_for_search() {
        let mut category_service = MockCategoryService::default();
        category_service.expect_search_categories().never();
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
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

        let response = handle(
            lambda_event,
            &category_service,
            &period_service,
            &query_product_service,
        )
        .await
        .unwrap_err();

        assert_eq!(400, response.status);
    }
}
