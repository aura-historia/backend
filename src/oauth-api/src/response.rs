use aws_lambda_events::apigw::ApiGatewayV2httpResponse;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use http::header::LOCATION;
use serde::Serialize;

pub fn redirect(location: &str) -> ApiGatewayV2httpResponse {
    let mut response = ApiGatewayV2HttpResponseBuilder::new(302)
        .cache_control("no-store", None, None)
        .build();
    response.headers.insert(
        LOCATION,
        location
            .parse()
            .expect("failed to parse redirect location as HeaderValue"),
    );
    response
}

pub fn json_no_store(
    status: i64,
    data: impl Serialize + std::fmt::Debug,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    Ok(ApiGatewayV2HttpResponseBuilder::json(status)
        .cache_control("no-store", None, None)
        .body_serde(data)?
        .build())
}
