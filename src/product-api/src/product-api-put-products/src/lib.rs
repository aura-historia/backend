use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::collection::PutCollectionData;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::BAD_BODY_VALUE;
use common::localized::Localized;
use common::price::domain::Price;
use product::data::put_data::PutItemData;
use product::service::enrichment_service::{EnrichItemCommandError, ItemCommandEnrichmentService};
use product::service::item_command::{PipedItemCommand, UpsertItemCommand};
use product::service::upsert_service::UpsertItemsService;
use lambda_runtime::LambdaEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::warn;
use url::Url;

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, into = "String", try_from = "String")]
pub enum PutItemError {
    #[error("SHOP_NOT_FOUND")]
    ShopNotFound,

    #[error("MONETARY_AMOUNT_OVERFLOW")]
    MonetaryAmountOverflow,

    #[error("ITEM_ENRICHMENT_FAILED")]
    EnrichmentError,
}

impl From<PutItemError> for String {
    fn from(err: PutItemError) -> String {
        err.to_string()
    }
}

impl TryFrom<String> for PutItemError {
    type Error = String;

    fn try_from(payload: String) -> Result<Self, Self::Error> {
        match payload.as_str() {
            "SHOP_NOT_FOUND" => Ok(PutItemError::ShopNotFound),
            "MONETARY_AMOUNT_OVERFLOW" => Ok(PutItemError::MonetaryAmountOverflow),
            "ITEM_ENRICHMENT_FAILED" => Ok(PutItemError::EnrichmentError),
            other => Err(format!(
                "Expected any of 'SHOP_NOT_FOUND', 'MONETARY_AMOUNT_OVERFLOW', 'ITEM_ENRICHMENT_FAILED'. Got '{other}'"
            )),
        }
    }
}

impl From<EnrichItemCommandError> for PutItemError {
    fn from(cmd_error: EnrichItemCommandError) -> Self {
        match cmd_error {
            EnrichItemCommandError::MonetaryAmountOverflowError(_) => {
                PutItemError::MonetaryAmountOverflow
            }
            EnrichItemCommandError::UnknownShopUrl(_) => PutItemError::ShopNotFound,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutItemsResponse {
    #[serde(default)]
    pub unprocessed: Vec<Url>,

    #[serde(default)]
    pub failed: HashMap<Url, PutItemError>,

    pub skipped: u64,
}

#[tracing::instrument(
    skip(event, upsert_service, enrich_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    upsert_service: &impl UpsertItemsService,
    enrich_service: &(impl ItemCommandEnrichmentService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, upsert_service, enrich_service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// PUT /api/v1/items
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    upsert_service: &impl UpsertItemsService,
    enrich_service: &(impl ItemCommandEnrichmentService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_message(err_msg)
        })?;
    let items_data: PutCollectionData<PutItemData> =
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_message(err_msg)
        })?;

    let commands = items_data
        .items
        .into_iter()
        .map(put_item_data_to_command)
        .collect::<Vec<_>>();

    let mut unprocessed = Vec::new();
    let mut failed = HashMap::new();

    let enriched_res = enrich_service.enrich(commands).await;
    unprocessed.extend(
        &mut enriched_res
            .unprocessed
            .into_iter()
            .map(|piped_cmd| piped_cmd.url),
    );
    failed.extend(
        &mut enriched_res
            .failed
            .into_iter()
            .map(|(piped_cmd, err)| (piped_cmd.url, PutItemError::from(err))),
    );
    let enriched_upsert_cmds = enriched_res
        .enriched
        .into_iter()
        .map(|piped_cmd| (piped_cmd.url.clone(), piped_cmd))
        .filter_map(
            |(url, piped_cmd)| match UpsertItemCommand::try_from(piped_cmd) {
                Ok(cmd) => Some(cmd),
                Err(err) => {
                    warn!(
                        error = %err,
                        fromType = %std::any::type_name::<PipedItemCommand>(),
                        toType = %std::any::type_name::<UpsertItemCommand>(),
                        "Failed mapping types."
                    );
                    unprocessed.push(url);
                    None
                }
            },
        )
        .collect();

    let upsert_res = upsert_service.upsert(enriched_upsert_cmds).await;
    unprocessed.extend(
        &mut upsert_res
            .unprocessed
            .into_iter()
            .map(|upsert_cmd| upsert_cmd.url),
    );

    let response_payload = PutItemsResponse {
        unprocessed,
        failed,
        skipped: upsert_res.skipped as u64,
    };
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(response_payload)?
        .build())
}

fn put_item_data_to_command(data: PutItemData) -> PipedItemCommand {
    PipedItemCommand {
        shop_id: None,
        shops_product_id: data.shops_product_id,
        shop_name: None,
        native_title: data.title.into(),
        other_title: Default::default(),
        native_description: data.description.map(Localized::from),
        other_description: Default::default(),
        native_price: data.price.map(Price::from),
        other_price: Default::default(),
        state: data.state.into(),
        url: data.url,
        images: data.images,
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::api::collection::PutCollectionData;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use product::data::put_data::PutItemData;
    use product::service::enrichment_service::{
        EnrichItemCommandsOutput, MockItemCommandEnrichmentService,
    };
    use product::service::item_command::UpsertItemCommand;
    use product::service::upsert_service::{MockUpsertItemsService, UpsertItemsOutput};
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
        let mut enrich_service = MockItemCommandEnrichmentService::default();
        enrich_service.expect_enrich().return_once(|cmds| {
            Box::pin(async {
                EnrichItemCommandsOutput {
                    enriched: cmds
                        .into_iter()
                        .map(|mut cmd| {
                            cmd.shop_id = Some(ShopId::new());
                            cmd.shop_name = Some(Faker.fake());
                            cmd
                        })
                        .collect(),
                    failed: vec![],
                    unprocessed: vec![],
                }
            })
        });
        let mut upsert_service = MockUpsertItemsService::default();
        upsert_service.expect_upsert().return_once(move |_| {
            Box::pin(async move {
                UpsertItemsOutput {
                    unprocessed: fake::vec![UpsertItemCommand; failures],
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
        let response = handler(lambda_event, &upsert_service, &enrich_service)
            .await
            .unwrap();

        let actual_json = extract_apigw_response_json_body!(response);
        assert_eq!(
            failures,
            actual_json["unprocessed"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .len()
        );
        assert_eq!(skipped, actual_json["skipped"]);
    }
}
