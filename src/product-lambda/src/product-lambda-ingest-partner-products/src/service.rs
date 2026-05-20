use crate::types::AsyncProductCommandData;
use async_trait::async_trait;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::types::SendMessageBatchRequestEntry;
use common::has_key::HasKey;
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
pub struct AsyncProductCommandFailure {
    pub command: AsyncProductCommandData,
    pub error: String,
}

#[async_trait]
#[mockall::automock]
pub trait AsyncProductCommandService {
    async fn send(&self, commands: Vec<AsyncProductCommandData>)
    -> Vec<AsyncProductCommandFailure>;
}

pub struct AsyncProductCommandServiceImpl<'a> {
    sqs: &'a Client,
    queue_url: String,
}

impl<'a> AsyncProductCommandServiceImpl<'a> {
    pub fn new(sqs: &'a Client, queue_url: impl Into<String>) -> Self {
        Self {
            sqs,
            queue_url: queue_url.into(),
        }
    }
}

#[async_trait]
impl AsyncProductCommandService for AsyncProductCommandServiceImpl<'_> {
    async fn send(
        &self,
        commands: Vec<AsyncProductCommandData>,
    ) -> Vec<AsyncProductCommandFailure> {
        let mut failures = Vec::new();

        for chunk in commands.chunks(10) {
            let chunk = chunk.to_vec();
            let mut entries = Vec::with_capacity(chunk.len());
            for (i, command) in chunk.iter().enumerate() {
                match serde_json::to_string(command) {
                    Ok(body) => entries.push(
                        SendMessageBatchRequestEntry::builder()
                            .id(i.to_string())
                            .message_body(body)
                            .build()
                            .expect("id and message_body are set"),
                    ),
                    Err(err) => failures.push(AsyncProductCommandFailure {
                        command: command.clone(),
                        error: err.to_string(),
                    }),
                }
            }

            if entries.is_empty() {
                continue;
            }

            match self
                .sqs
                .send_message_batch()
                .queue_url(&self.queue_url)
                .set_entries(Some(entries))
                .send()
                .await
            {
                Ok(output) => {
                    for successful in output.successful {
                        if let Ok(index) = successful.id.parse::<usize>()
                            && let Some(command) = chunk.get(index)
                        {
                            let key = command.key();
                            info!(
                                productCommandIntent = %command.intent(),
                                shopId = %key.shop_id,
                                shopsProductId = %key.shops_product_id,
                                "Forwarded partner product command to async ingestion queue."
                            );
                        }
                    }
                    for failed in output.failed {
                        if let Ok(index) = failed.id.parse::<usize>()
                            && let Some(command) = chunk.get(index)
                        {
                            failures.push(AsyncProductCommandFailure {
                                command: command.clone(),
                                error: failed
                                    .message
                                    .unwrap_or_else(|| "SQS send failed".to_string()),
                            });
                        }
                    }
                }
                Err(err) => {
                    let error = err.to_string();
                    failures.extend(chunk.into_iter().map(|command| AsyncProductCommandFailure {
                        command,
                        error: error.clone(),
                    }));
                }
            }
        }

        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::UpsertAsyncProductCommandData;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;

    fn command(id: &str) -> AsyncProductCommandData {
        AsyncProductCommandData::Upsert(UpsertAsyncProductCommandData {
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::from(id.to_string()),
            title: None,
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            seller_name: None,
            structured_address: None,
            geo_address: None,
        })
    }

    #[tokio::test]
    async fn should_return_no_failures_when_no_commands_for_service() {
        let config = aws_sdk_sqs::Config::builder()
            .behavior_version_latest()
            .build();
        let client = Client::from_conf(config);
        let service = AsyncProductCommandServiceImpl::new(&client, "queue-url");

        let failures = service.send(vec![]).await;

        assert!(failures.is_empty());
    }

    #[test]
    fn should_keep_command_in_failure_for_service() {
        let failure = AsyncProductCommandFailure {
            command: command("product-1"),
            error: "failed".to_string(),
        };

        assert_eq!(
            "product-1",
            failure.command.key().shops_product_id.to_string()
        );
    }
}
