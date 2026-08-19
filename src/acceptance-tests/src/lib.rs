use aws_sdk_dynamodb::types::WriteRequest;
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use aws_tests_common::get_cfn_output;
use opensearch::http::response::Response;
use std::{collections::HashMap, error::Error};
pub use test_api::aura_integration_test;
use test_api::{get_dynamodb_client, get_opensearch_client, get_sqs_client};
use tracing::debug;

// Called before each test to ensure clean state
pub async fn reset() {
    let cfn_output = get_cfn_output().clone();
    clear_ddb_table_data()
        .await
        .expect("shouldn't fail clearing table-data");
    clear_os_index_data("products")
        .await
        .expect("shouldn't fail clearing os-index 'products'");
    clear_os_index_data("shops")
        .await
        .expect("shouldn't fail clearing os-index 'shops'");
    clear_os_index_data("users")
        .await
        .expect("shouldn't fail clearing os-index 'users'");
    clear_os_index_data("user_search_filters")
        .await
        .expect("shouldn't fail clearing os-index 'user_search_filter'");
    clear_qs(vec![
        cfn_output.product_materialize_opensearch_queue_url,
        cfn_output.product_materialize_opensearch_dead_letter_queue_url,
        cfn_output.product_delete_product_queue_url,
        cfn_output.product_delete_product_dead_letter_queue_url,
        cfn_output.product_partner_ingest_queue_url,
        cfn_output.product_partner_ingest_dead_letter_queue_url,
        cfn_output.shop_opensearch_index_queue_url,
        cfn_output.shop_opensearch_index_dead_letter_queue_url,
        cfn_output.user_opensearch_index_queue_url,
        cfn_output.user_opensearch_index_dead_letter_queue_url,
        cfn_output.search_filter_open_search_sync_queue_url,
        cfn_output.search_filter_open_search_sync_dead_letter_queue_url,
        cfn_output.product_update_notify_user_queue_url,
        cfn_output.product_update_notify_user_dead_letter_queue_url,
    ])
    .await
    .expect("shouldn't fail clearing queues");
}

/// Clears all items from the DynamoDB table to ensure test isolation.
///
/// This function scans the table and deletes all items in batches.
async fn clear_ddb_table_data() -> Result<(), Box<dyn Error>> {
    use aws_sdk_dynamodb::types::{AttributeValue, DeleteRequest};

    let client = get_dynamodb_client().await;

    // Scan the table to get all items
    let mut exclusive_start_key: Option<HashMap<String, AttributeValue>> = None;

    loop {
        let mut scan_request = client
            .scan()
            .table_name(get_cfn_output().dynamodb_table_1_name.clone());

        if let Some(start_key) = exclusive_start_key {
            scan_request = scan_request.set_exclusive_start_key(Some(start_key));
        }

        let scan_output = scan_request.consistent_read(true).send().await?;

        if let Some(items) = scan_output.items
            && !items.is_empty()
        {
            // Delete items in batches
            let delete_requests: Vec<WriteRequest> = items
                .into_iter()
                .map(|item| {
                    let mut key = HashMap::new();
                    key.insert("pk".to_string(), item.get("pk").unwrap().clone());
                    key.insert("sk".to_string(), item.get("sk").unwrap().clone());

                    WriteRequest::builder()
                        .delete_request(
                            DeleteRequest::builder().set_key(Some(key)).build().unwrap(),
                        )
                        .build()
                })
                .collect();

            // Process deletes in batches of 25 (DynamoDB limit)
            for chunk in delete_requests.chunks(25) {
                let mut request_items = HashMap::new();
                request_items.insert(
                    get_cfn_output().dynamodb_table_1_name.clone(),
                    chunk.to_vec(),
                );

                client
                    .batch_write_item()
                    .set_request_items(Some(request_items))
                    .send()
                    .await?;
                debug!("Cleared a chunk of size '{}' from table", chunk.len());
            }
        }

        // Check if there are more items to scan
        exclusive_start_key = scan_output.last_evaluated_key;
        if exclusive_start_key.is_none() {
            break;
        }
    }

    debug!(
        "Cleared table '{}'.",
        get_cfn_output().dynamodb_table_1_name
    );

    Ok(())
}

async fn clear_os_index_data(index: &str) -> Result<Response, opensearch::Error> {
    use opensearch::DeleteByQueryParts;
    use serde_json::json;

    let query = json!({
        "query": {
            "match_all": {}
        }
    });

    let res = get_opensearch_client()
        .await
        .delete_by_query(DeleteByQueryParts::Index(&[index]))
        .body(query)
        .refresh(true)
        .send()
        .await?
        .error_for_status_code()?;

    debug!("Cleared index '{index}'.");

    Ok(res)
}

// Manually deleting in batches as purging introduces 60s no-op window
async fn clear_q(queue_url: String) -> Result<(), Box<dyn Error>> {
    let client = get_sqs_client().await;
    loop {
        let resp = client
            .receive_message()
            .queue_url(queue_url.clone())
            .max_number_of_messages(10)
            .wait_time_seconds(1)
            .send()
            .await?;

        let messages = resp.messages.unwrap_or_default();
        if messages.is_empty() {
            break;
        }

        let entries: Vec<_> = messages
            .into_iter()
            .filter_map(|m| {
                m.receipt_handle.map(|handle| {
                    DeleteMessageBatchRequestEntry::builder()
                        .id(uuid::Uuid::new_v4().to_string())
                        .receipt_handle(handle)
                        .build()
                        .unwrap()
                })
            })
            .collect();

        client
            .delete_message_batch()
            .queue_url(queue_url.clone())
            .set_entries(Some(entries.clone()))
            .send()
            .await?;
        debug!(
            "Removed batch of size '{}' from queue '{}'.",
            entries.len(),
            queue_url
        );
    }

    debug!("Cleared queue '{queue_url}'.");

    Ok(())
}

async fn clear_qs(queue_urls: Vec<String>) -> Result<(), Box<dyn Error>> {
    for queue_url in queue_urls {
        clear_q(queue_url).await?;
    }
    Ok(())
}
