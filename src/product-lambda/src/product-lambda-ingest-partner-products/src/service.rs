use crate::types::AsyncProductCommandData;
use async_trait::async_trait;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::types::SendMessageBatchRequestEntry;
use common::has_key::HasKey;
use product::service::command_service::CommandProductService;
use product::service::product_command::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use std::collections::{HashMap, HashSet};
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
        info!(
            count = commands.len(),
            "Forwarding product commands to SQS for background processing."
        );
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
                    for failed in output.failed {
                        if let Ok(index) = failed.id.parse::<usize>() {
                            if let Some(command) = chunk.get(index) {
                                failures.push(AsyncProductCommandFailure {
                                    command: command.clone(),
                                    error: failed
                                        .message
                                        .unwrap_or_else(|| "SQS send failed".to_string()),
                                });
                            }
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

#[async_trait]
impl<T> AsyncProductCommandService for T
where
    T: CommandProductService + Sync,
{
    async fn send(
        &self,
        commands: Vec<AsyncProductCommandData>,
    ) -> Vec<AsyncProductCommandFailure> {
        let mut failures = Vec::new();

        let creates: Vec<CreateProductCommand> = commands
            .iter()
            .filter_map(|command| match command {
                AsyncProductCommandData::Create(data) => Some(data.clone().into()),
                _ => None,
            })
            .collect();
        let failed_create_keys: HashSet<_> = if creates.is_empty() {
            HashSet::new()
        } else {
            self.create(creates)
                .await
                .into_iter()
                .map(|command| command.key())
                .collect()
        };

        let updates: HashMap<_, UpdateProductCommand> = commands
            .iter()
            .filter_map(|command| match command {
                AsyncProductCommandData::Update(data) => Some(data.clone().into()),
                _ => None,
            })
            .collect();
        let failed_update_keys: HashSet<_> = if updates.is_empty() {
            HashSet::new()
        } else {
            self.update(updates).await.into_keys().collect()
        };

        let upserts: Vec<UpsertProductCommand> = commands
            .iter()
            .filter_map(|command| match command {
                AsyncProductCommandData::Upsert(data) => Some(data.clone().into()),
                _ => None,
            })
            .collect();
        let failed_upsert_keys: HashSet<_> = if upserts.is_empty() {
            HashSet::new()
        } else {
            self.upsert(upserts)
                .await
                .into_iter()
                .map(|command| command.key())
                .collect()
        };

        for command in commands {
            let failed = match &command {
                AsyncProductCommandData::Create(_) => failed_create_keys.contains(&command.key()),
                AsyncProductCommandData::Update(_) => failed_update_keys.contains(&command.key()),
                AsyncProductCommandData::Upsert(_) => failed_upsert_keys.contains(&command.key()),
            };
            if failed {
                failures.push(AsyncProductCommandFailure {
                    command,
                    error: "PRODUCT_COMMAND_FAILED".to_string(),
                });
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

        assert_eq!("product-1", failure.command.shops_product_id().to_string());
    }
}
