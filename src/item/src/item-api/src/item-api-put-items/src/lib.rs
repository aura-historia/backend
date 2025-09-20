use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use aws_lambda_events::query_map::QueryMap;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::collection::PutCollectionData;
use common::api::error::ApiError;
use common::api::error_code::{
    BAD_BODY_VALUE, BAD_PATH_PARAMETER_VALUE, BAD_QUERY_PARAMETER_VALUE,
};
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_data::get_data::GetItemData;
use item_data::put_data::PutItemData;
use item_service::get_service::GetItemService;
use lambda_runtime::LambdaEvent;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetItemService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// PUT /api/v1/items
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetItemService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE).with_message("Body cannot be empty")
        })?;
    let items_data: PutCollectionData<PutItemData> = serde_json::from_str(&body)
        .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string()))?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200).cors().build())
}
