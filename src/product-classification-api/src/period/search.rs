use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::sort::api::extract_sort_query;
use lambda_runtime::LambdaEvent;
use product_classification::period::data::get_period_summary_data::GetPeriodSummaryData;
use product_classification::period::data::sort_period_field_data::SortPeriodFieldData;
use product_classification::period::period_search::PeriodSearchData;
use product_classification::period::service::PeriodService;
use product_classification::period::sort_period_field::SortPeriodField;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PeriodService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let sort = extract_sort_query::<SortPeriodFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortPeriodField::from));

    let search_data: PeriodSearchData = if event.payload.route_key.as_deref()
        == Some("GET /api/v1/periods")
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

    let periods = service.search_periods(&search_data.into(), &sort).await?;
    let periods_data: Vec<GetPeriodSummaryData> = periods
        .into_iter()
        .map(GetPeriodSummaryData::from)
        .collect();

    let response_builder = ApiGatewayV2HttpResponseBuilder::json(200);
    let response_builder = if event.payload.route_key.as_deref() == Some("GET /api/v1/periods") {
        response_builder.cache_control("public", Some(86400), Some(604800))
    } else {
        response_builder
    };

    Ok(response_builder.body_serde(periods_data)?.build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use lambda_runtime::LambdaEvent;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::core::Period;
    use product_classification::period::period_search::PeriodSearchData;
    use product_classification::period::service::MockPeriodService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_search_periods_when_name_query_present_for_search() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_search_periods()
            .return_once(move |search, _| {
                let periods: Vec<Period> = fake::vec![Period; 2];
                let localized = periods
                    .into_iter()
                    .map(|p| p.localized(&[search.language]))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let search_data = PeriodSearchData {
            language: common::language::data::LanguageData::Es,
            name_query: Some("Renaissance".try_into().unwrap()),
        };
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/periods/search")
                .body_serde(&search_data)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_search_periods_when_get_query_params_for_search() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_search_periods()
            .return_once(move |search, _| {
                assert_eq!(common::language::domain::Language::Es, search.language);
                assert_eq!(Some("Renaissance".try_into().unwrap()), search.name_query);
                Box::pin(async move { Ok(vec![]) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/periods")
                .query_string_parameter("language", "es")
                .query_string_parameter("nameQuery", "Renaissance")
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
                .get(http::header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_search_periods_when_empty_search_for_search() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_search_periods()
            .return_once(move |search, _| {
                let periods: Vec<Period> = fake::vec![Period; 3];
                let localized = periods
                    .into_iter()
                    .map(|p| p.localized(&[search.language]))
                    .collect();
                Box::pin(async move { Ok(localized) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/periods/search")
                .body_serde(&PeriodSearchData::default())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_is_missing_for_search() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service.expect_search_periods().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/periods/search")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap_err();

        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_invalid_json_for_search() {
        let category_service = MockCategoryService::default();
        let mut period_service = MockPeriodService::default();
        period_service.expect_search_periods().never();
        let mut payload: aws_lambda_events::apigw::ApiGatewayV2httpRequest =
            ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/periods/search")
                .build();
        payload.body = Some("invalid-json".to_string());
        let lambda_event = LambdaEvent {
            payload,
            context: Default::default(),
        };

        let response = handle(lambda_event, &category_service, &period_service)
            .await
            .unwrap_err();

        assert_eq!(400, response.status);
    }
}
