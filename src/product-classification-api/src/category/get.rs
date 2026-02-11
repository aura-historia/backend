use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_PATH_PARAMETER_VALUE, NOT_FOUND};
use common::error::missing_field::MissingRequiredField;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use product_classification::category::data::get_category_data::GetCategoryData;
use product_classification::category::service::{CategoryService, CategoryServiceError};

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CategoryService,
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

    let languages: Vec<Language> = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect();

    let category = service
        .view_category(&category_id, &languages)
        .await
        .map_err(|err| match err {
            CategoryServiceError::CategoryNotExists(id) => {
                ApiError::not_found(NOT_FOUND, format!("Category '{id}' not found").into())
                    .with_path_field("categoryId")
                    .with_detail(format!("Category '{id}' not found"))
            }
            other => ApiError::from(other),
        })?;

    let category_data = GetCategoryData::from(category);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(category_data.updated)
        .body_serde(category_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::category_key::CategoryId;
    use fake::{Fake, Faker};
    use http::header::LAST_MODIFIED;
    use lambda_runtime::LambdaEvent;
    use product_classification::category::core::Category;
    use product_classification::category::service::{CategoryServiceError, MockCategoryService};
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;

    #[tokio::test]
    async fn should_return_category_when_exists_for_get_category() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockCategoryService::default();
        service
            .expect_view_category()
            .return_once(move |_, languages| {
                let mut category: Category = Faker.fake();
                category.updated = timestamp;
                let localized = category.localized(languages);
                Box::pin(async move { Ok(localized) })
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

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_path_param_category_id_is_missing_for_get_category() {
        let mut service = MockCategoryService::default();
        service.expect_view_category().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories/{categoryId}")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_404_when_category_does_not_exist_for_get_category() {
        let category_id: CategoryId = "missing-category".into();
        let mut service = MockCategoryService::default();
        service
            .expect_view_category()
            .return_once(move |category_id, _| {
                let category_id = category_id.clone();
                Box::pin(async move { Err(CategoryServiceError::CategoryNotExists(category_id)) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories/{categoryId}")
                .path_parameter("categoryId", category_id.to_string())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(404, response.status);
    }
}
