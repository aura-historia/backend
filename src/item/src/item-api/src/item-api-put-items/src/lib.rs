use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::collection::PutCollectionData;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::localized::Localized;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_data::put_data::PutItemData;
use item_service::command_service::{PutItemsOutput, PutItemsService};
use item_service::item_command::PutItemCommand;
use lambda_runtime::LambdaEvent;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnprocessedPutItem {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
}

#[derive(Debug, Clone, Serialize)]
pub struct PutItemsResponse {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unprocessed: Vec<UnprocessedPutItem>,
    pub skipped: u64,
}

impl From<PutItemsOutput> for PutItemsResponse {
    fn from(output: PutItemsOutput) -> Self {
        PutItemsResponse {
            unprocessed: output
                .unprocessed
                .into_iter()
                .map(|cmd| UnprocessedPutItem {
                    shop_id: cmd.shop_id,
                    shops_item_id: cmd.shops_item_id,
                })
                .collect(),
            skipped: output.skipped as u64,
        }
    }
}

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
    service: &impl PutItemsService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// PUT /api/v1/items
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PutItemsService,
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

    let commands = items_data
        .items
        .into_iter()
        .map(put_item_data_to_command)
        .collect::<Vec<_>>();
    let unprocessed = service.put(commands).await;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(PutItemsResponse::from(unprocessed))?
        .cors()
        .build())
}

fn put_item_data_to_command(data: PutItemData) -> PutItemCommand {
    PutItemCommand {
        shop_id: data.shop_id,
        shops_item_id: data.shops_item_id,
        shop_name: data.shop_name.into(),
        title: data.title.into(),
        description: data.description.map(Localized::from),
        price: data.price.map(Price::from),
        state: data.state.into(),
        url: data.url,
        images: data.images,
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::api::collection::PutCollectionData;
    use item_data::put_data::PutItemData;
    use item_service::command_service::{MockPutItemsService, PutItemsOutput};
    use item_service::item_command::PutItemCommand;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use test_api::extract_apigw_response_json_body;

    #[rstest::rstest]
    #[case(0, 0)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(7, 10)]
    #[case(24, 25)]
    #[case(0, 47)]
    #[case(98, 100)]
    #[case(1, 150)]
    #[case(0, 453)]
    #[case(0, 900)]
    #[case(2874, 2874)]
    #[case(874, 10874)]
    #[case(10874, 874)]
    #[tokio::test]
    async fn should_forward_failures_and_skipped_from_service(
        #[case] failures: usize,
        #[case] skipped: usize,
    ) {
        let mut service = MockPutItemsService::default();
        service.expect_put().return_once(move |_| {
            Box::pin(async move {
                PutItemsOutput {
                    unprocessed: fake::vec![PutItemCommand; failures],
                    skipped,
                }
            })
        });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .body_serde(&PutCollectionData {
                    items: fake::vec![PutItemData; 1234],
                })
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();

        let actual_json = extract_apigw_response_json_body!(response);
        if failures == 0 {
            assert!(actual_json.get("unprocessed").is_none())
        } else {
            assert_eq!(
                failures,
                actual_json["unprocessed"].as_array().unwrap().len()
            );
        }
        assert_eq!(skipped, actual_json["skipped"]);
    }
}
