use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use product_classification::period::data::get_period_summary_data::GetPeriodSummaryData;
use product_classification::period::service::PeriodService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PeriodService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages: Vec<Language> = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect();

    let periods = service.view_periods(&languages).await?;

    let periods_data: Vec<GetPeriodSummaryData> = periods
        .into_iter()
        .map(GetPeriodSummaryData::from)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("public", Some(86400), Some(604800))
        .body_serde(periods_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::core::Period;
    use product_classification::period::service::MockPeriodService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_return_all_periods_for_get_all() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_view_periods()
            .return_once(move |languages| {
                let periods: Vec<Period> = fake::vec![Period; 3];
                let localized = periods
                    .into_iter()
                    .map(|p| p.localized(languages))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_empty_list_when_no_periods_for_get_all() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_view_periods()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_with_long_max_ages_for_get_all_periods() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_view_periods()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods")
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
