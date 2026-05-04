pub mod service;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    batch::{Batch, dynamodb::handle_dynamodb_batch_write_put_product_output},
    dynamodb_stream::extract_from_dynamodb_stream,
    event_id::EventId,
    has_key::HasKey,
    language::domain::Language,
    product_id::{ProductId, ProductKey},
};
use lambda_runtime::LambdaEvent;
use product::{
    core::{
        product_event::{
            ProductEvent, ProductEventPayload,
            enrichment::{ProductEnrichmentEventPayload, TranslationProductEnrichmentEventPayload},
        },
        title::Title,
    },
    dynamodb::{product_event_record::ProductEventRecord, repository::ProductDynamoDbRepository},
    service::get_service::GetProductService,
};
use service::TranslationService;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(translation_service, get_product_service, product_repository, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    translation_service: &(impl TranslationService + Sync),
    get_product_service: &(impl GetProductService + Sync),
    product_repository: &(impl ProductDynamoDbRepository + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    let mut key_to_record: HashMap<ProductKey, (String, ProductEventRecord)> = HashMap::new();

    for (message_id, event_record) in event_records {
        match event_record {
            ProductEventRecord::Domain(ref domain_record) => {
                let key = ProductKey::new(
                    domain_record.shop_id,
                    domain_record.shops_product_id.clone(),
                );
                key_to_record.insert(key, (message_id, event_record));
            }
            other => {
                let key = other.key();
                error!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    eventId = %other.event_id(),
                    "Unexpected non-Domain event record type in translate handler."
                );
            }
        }
    }

    if key_to_record.is_empty() {
        let failures = failed_message_ids.len();
        info!(successful = 0, failures = failures, "Handler finished.");
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    let keys: Vec<ProductKey> = key_to_record.keys().cloned().collect();
    let products = match get_product_service.find_products(keys).await {
        Ok(ps) => ps,
        Err(err) => {
            error!(error = ?err, "Failed batch-loading products from DynamoDB — marking all messages as failed.");
            failed_message_ids.extend(key_to_record.into_values().map(|(msg_id, _)| msg_id));
            let failures = failed_message_ids.len();
            info!(successful = 0, failures = failures, "Handler finished.");
            let mut sqs_batch_response = SqsBatchResponse::default();
            sqs_batch_response.batch_item_failures = failed_message_ids
                .into_iter()
                .map(mk_batch_item_failure)
                .collect();
            return Ok(sqs_batch_response);
        }
    };

    struct ProductInput {
        message_id: String,
        product_id: ProductId,
        key: ProductKey,
        seller_id: common::shop_id::ShopId,
        source_language: Language,
        title: String,
    }

    let mut valid_inputs: Vec<ProductInput> = Vec::new();

    for product in products {
        let key = ProductKey::new(product.shop_id, product.shops_product_id.clone());
        let (message_id, _) = match key_to_record.remove(&key) {
            Some(v) => v,
            None => {
                warn!(
                    shopId = %product.shop_id,
                    shopsProductId = %product.shops_product_id,
                    "Loaded product has no corresponding SQS message — skipping."
                );
                continue;
            }
        };

        let title_trimmed = product.native_title.payload.as_ref().trim();
        if title_trimmed.is_empty() {
            warn!(
                messageId = message_id,
                shopId = %product.shop_id,
                shopsProductId = %product.shops_product_id,
                "Product has empty native title — skipping translation."
            );
            continue;
        }

        valid_inputs.push(ProductInput {
            message_id,
            product_id: product.product_id,
            seller_id: product.seller_id,
            source_language: product.native_title.localization,
            title: title_trimmed.to_string(),
            key,
        });
    }

    for (message_id, record) in key_to_record.values() {
        let key = record.key();
        error!(
            messageId = message_id,
            shopId = %key.shop_id,
            shopsProductId = %key.shops_product_id,
            "Materialized product not found in DynamoDB — marking message as failed for retry."
        );
        failed_message_ids.push(message_id.clone());
    }

    if valid_inputs.is_empty() {
        let failures = failed_message_ids.len();
        info!(successful = 0, failures = failures, "Handler finished.");
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    let mut by_language: HashMap<Language, Vec<usize>> = HashMap::new();
    for (idx, input) in valid_inputs.iter().enumerate() {
        by_language
            .entry(input.source_language)
            .or_default()
            .push(idx);
    }

    let mut translation_results: Vec<(usize, Option<HashMap<Language, String>>)> =
        Vec::with_capacity(valid_inputs.len());

    for (source_language, indices) in by_language {
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_by_key(|&i| valid_inputs[i].title.len());

        let titles: Vec<String> = sorted_indices
            .iter()
            .map(|&i| valid_inputs[i].title.clone())
            .collect();

        let group_results = translation_service
            .translate(&titles, source_language)
            .await;

        for (batch_pos, &original_idx) in sorted_indices.iter().enumerate() {
            translation_results.push((
                original_idx,
                group_results.get(batch_pos).cloned().flatten(),
            ));
        }
    }

    let mut enrichment_events: Vec<(String, ProductEventRecord)> = Vec::new();

    for (idx, maybe_translations) in translation_results {
        let input = &valid_inputs[idx];
        let translations = match maybe_translations {
            Some(t) => t,
            None => {
                error!(
                    messageId = input.message_id,
                    shopId = %input.key.shop_id,
                    shopsProductId = %input.key.shops_product_id,
                    "Translation service returned no result for product — marking message as failed."
                );
                failed_message_ids.push(input.message_id.clone());
                continue;
            }
        };

        let now = OffsetDateTime::now_utc();

        for (target_language, translated_title) in translations {
            let title_event = ProductEvent {
                aggregate_id: input.product_id,
                event_id: EventId::new(),
                timestamp: now,
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::TranslatedTitle(
                        TranslationProductEnrichmentEventPayload {
                            shop_id: input.key.shop_id,
                            seller_id: input.seller_id,
                            shops_product_id: input.key.shops_product_id.clone(),
                            source_language: input.source_language,
                            target_language,
                            target: Title::from(translated_title),
                        },
                    ),
                ),
            };
            let title_record: ProductEventRecord = title_event.into();
            enrichment_events.push((input.message_id.clone(), title_record));
        }
    }

    persist_events(
        product_repository,
        enrichment_events,
        &mut failed_message_ids,
    )
    .await;

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures,
        failures = failures,
        "Handler finished.",
    );

    let mut sqs_batch_response = SqsBatchResponse::default();
    sqs_batch_response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(mk_batch_item_failure)
        .collect();
    Ok(sqs_batch_response)
}

fn mk_batch_item_failure(item_identifier: String) -> BatchItemFailure {
    let mut failure = BatchItemFailure::default();
    failure.item_identifier = item_identifier;
    failure
}

async fn persist_events(
    repository: &(impl ProductDynamoDbRepository + Sync),
    events: Vec<(String, ProductEventRecord)>,
    failed_message_ids: &mut Vec<String>,
) {
    for batch in Batch::chunked_from(events.into_iter()) {
        let batch: Batch<_, 25> = batch;
        let batch_message_ids = batch
            .iter()
            .map(|(message_id, record)| (record.key(), message_id.clone()))
            .collect::<HashMap<ProductKey, String>>();
        let batch = Batch::try_from_iter(batch.into_iter().map(|(_, record)| record))
            .expect("shouldn't fail re-building batch of same size from former batch");
        match repository.put_product_event_records(batch).await {
            Ok(output) => {
                let mut failures = Vec::new();
                handle_dynamodb_batch_write_put_product_output::<ProductEventRecord>(
                    output,
                    &mut failures,
                );
                for key in failures {
                    match batch_message_ids.get(&key) {
                        Some(message_id) => failed_message_ids.push(message_id.clone()),
                        None => {
                            error!(
                                productKey = %key,
                                "There exists no message_id for failed ProductEventRecord."
                            );
                        }
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed entire event batch.");
                failed_message_ids.extend(batch_message_ids.into_values());
            }
        }
    }
}
