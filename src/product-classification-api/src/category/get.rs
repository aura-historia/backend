use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_PATH_PARAMETER_VALUE;
use common::error::missing_field::MissingRequiredField;
use common::language::data::api::extract_language_query;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::query::any_of_query::AnyOfQuery;
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::query_service::QueryProductService;
use product_classification::category::data::get_category_data::GetCategoryData;
use product_classification::category::service::CategoryService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    category_service: &impl CategoryService,
    query_product_service: &impl QueryProductService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let category_id = event
        .payload
        .path_parameters
        .get("categoryId")
        .map(|s| s.into())
        .ok_or_else(|| {
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("categoryId")),
            )
            .with_path_field("categoryId")
            .with_detail("Missing field 'categoryId'.")
        })?;

    let languages = vec![Language::from(extract_language_query(
        &event.payload.query_string_parameters,
    )?)];

    let category = category_service
        .view_category(&category_id, &languages)
        .await?;
    let product_search =
        ProductSearch::default().with_category_id(AnyOfQuery::from_iter([category_id]));
    let cursor = Cursor {
        search_after: None,
        size: 0,
    };
    let products = query_product_service
        .search_products(&product_search, &None, &Some(cursor))
        .await?;

    let category_data = GetCategoryData::from_category_with_product_count(
        category,
        products.total.unwrap_or(0) as u32,
    );

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(category_data.updated)
        .cache_control("public", Some(600), Some(3600))
        .body_serde(category_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::category_key::CategoryId;
    use common::pagination::cursor::{Cursor, CursoredResult};
    use fake::{Fake, Faker};
    use http::header::{CACHE_CONTROL, LAST_MODIFIED};
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::service::query_service::MockQueryProductService;
    use product_classification::category::core::Category;
    use product_classification::category::service::{CategoryServiceError, MockCategoryService};
    use product_classification::period::service::MockPeriodService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;

    #[tokio::test]
    async fn should_return_category_when_exists_for_get_category() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_view_category()
            .return_once(move |_, languages| {
                let mut category: Category = Faker.fake();
                category.updated = timestamp;
                let localized = category.localized(languages);
                Box::pin(async move { Ok(localized) })
            });
        let period_service = MockPeriodService::default();
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products()
            .return_once(|_, _, _| {
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
        let category_id: CategoryId = "test-category".into();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories/{categoryId}")
                .path_parameter("categoryId", category_id.to_string())
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
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_path_param_category_id_is_missing_for_get_category() {
        let mut category_service = MockCategoryService::default();
        category_service.expect_view_category().never();
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories/{categoryId}")
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
    async fn should_404_when_category_does_not_exist_for_get_category() {
        let category_id: CategoryId = "missing-category".into();
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_view_category()
            .return_once(move |category_id, _| {
                let category_id = category_id.clone();
                Box::pin(async move { Err(CategoryServiceError::CategoryNotExists(category_id)) })
            });
        let period_service = MockPeriodService::default();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories/{categoryId}")
                .path_parameter("categoryId", category_id.to_string())
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

        assert_eq!(404, response.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_with_long_max_ages_for_get_category() {
        let category_id: CategoryId = "test-category".into();
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_view_category()
            .return_once(move |_, languages| {
                let category: Category = Faker.fake();
                let localized = category.localized(languages);
                Box::pin(async move { Ok(localized) })
            });
        let period_service = MockPeriodService::default();
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products()
            .return_once(|_, _, _| {
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
                .route_key("GET /api/v1/categories/{categoryId}")
                .path_parameter("categoryId", category_id.to_string())
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
