use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_PATH_PARAMETER_VALUE;
use common::error::missing_field::MissingRequiredField;
use common::language::data::api::extract_language_query;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use product_classification::period::data::get_period_data::GetPeriodData;
use product_classification::period::service::PeriodService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PeriodService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let period_id = event
        .payload
        .path_parameters
        .get("periodId")
        .map(|s| s.into())
        .ok_or_else(|| {
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("periodId")),
            )
            .with_path_field("periodId")
            .with_detail("Missing field 'periodId'.")
        })?;

    let languages = vec![Language::from(extract_language_query(
        &event.payload.query_string_parameters,
    )?)];

    let period = service.view_period(&period_id, &languages).await?;

    let period_data = GetPeriodData::from(period);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(period_data.updated)
        .cache_control("public", Some(3600), Some(86400))
        .body_serde(period_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::period_key::PeriodId;
    use fake::{Fake, Faker};
    use http::header::{CACHE_CONTROL, LAST_MODIFIED};
    use lambda_runtime::LambdaEvent;
    use product::service::query_service::MockQueryProductService;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::core::Period;
    use product_classification::period::service::{MockPeriodService, PeriodServiceError};
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;

    #[tokio::test]
    async fn should_return_period_when_exists_for_get_period() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_view_period()
            .return_once(move |_, languages| {
                let mut period: Period = Faker.fake();
                period.updated = timestamp;
                let localized = period.localized(languages);
                Box::pin(async move { Ok(localized) })
            });
        let query_product_service = MockQueryProductService::default();
        let period_id: PeriodId = "test-period".into();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods/{periodId}")
                .path_parameter("periodId", period_id.to_string())
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
    async fn should_400_when_path_param_period_id_is_missing_for_get_period() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service.expect_view_period().never();
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods/{periodId}")
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
    async fn should_404_when_period_does_not_exist_for_get_period() {
        let category_service = MockCategoryService::default();
        let period_id: PeriodId = "missing-period".into();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_view_period()
            .return_once(move |period_id, _| {
                let period_id = period_id.clone();
                Box::pin(async move { Err(PeriodServiceError::PeriodNotExists(period_id)) })
            });
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods/{periodId}")
                .path_parameter("periodId", period_id.to_string())
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
    async fn should_set_cache_control_to_public_with_long_max_ages_for_get_period() {
        let category_service = MockCategoryService::default();
        let period_id: PeriodId = "test-period".into();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_view_period()
            .return_once(move |_, languages| {
                let period: Period = Faker.fake();
                let localized = period.localized(languages);
                Box::pin(async move { Ok(localized) })
            });
        let query_product_service = MockQueryProductService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods/{periodId}")
                .path_parameter("periodId", period_id.to_string())
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
            "public, max-age=3600, s-maxage=86400",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
