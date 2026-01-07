use crate::payload::MailPayload;
use aws_sdk_sqs::types::SendMessageBatchRequestEntry;
use common::batch::Batch;
use std::collections::HashMap;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait QueueMailService {
    async fn queue_mails(&self, payloads: Vec<MailPayload>) -> Vec<MailPayload>;
}

#[derive(Debug, Clone)]
pub struct QueueMailServiceImpl<'a> {
    sqs_client: &'a aws_sdk_sqs::Client,
    mail_queue_url: &'a str,
}

impl<'a> QueueMailServiceImpl<'a> {
    pub fn new(sqs_client: &'a aws_sdk_sqs::Client, mail_queue_url: &'a str) -> Self {
        Self {
            sqs_client,
            mail_queue_url,
        }
    }
}

#[async_trait::async_trait]
impl<'a> QueueMailService for QueueMailServiceImpl<'a> {
    async fn queue_mails(&self, payloads: Vec<MailPayload>) -> Vec<MailPayload> {
        let mut failures = Vec::new();
        let batches = Batch::<_, 10>::chunked_from(payloads.into_iter());

        for batch in batches {
            let mut msg_payload = batch
                .iter()
                .enumerate()
                .map(|(i, payload)| (i.to_string(), payload.clone()))
                .collect::<HashMap<_, _>>();
            let message_entries = batch
                .into_iter()
                .enumerate()
                .filter_map(|(i, m)| match serde_json::to_string(&m) {
                    Ok(payload) => Some(
                        SendMessageBatchRequestEntry::builder()
                            .message_body(payload)
                            .id(i.to_string())
                            .message_deduplication_id(m.mail_id)
                            .message_group_id(m.user_id)
                            .build()
                            .expect("shouldn't fail because 'id' and 'message_body' have been set"),
                    ),
                    Err(err) => {
                        error!(
                            error = %err,
                            userId= %m.user_id,
                            mailId= %m.mail_id,
                            type = %std::any::type_name::<MailPayload>(),
                            "Failed to serialize message."
                        );
                        None
                    }
                })
                .collect();
            let res = self
                .sqs_client
                .send_message_batch()
                .queue_url(self.mail_queue_url)
                .set_entries(Some(message_entries))
                .send()
                .await;
            match res {
                Ok(output) => {
                    for failed in output.failed {
                        match msg_payload.remove(failed.id()) {
                            Some(failed_payload) => failures.push(failed_payload),
                            None => error!(
                                payload = ?failed,
                                "Couldn't find MailPayload for unprocessed message. This is a bug. Not retrying."
                            ),
                        }
                    }
                }
                Err(err) => {
                    error!(error = ?err, "Failed writing entire MailPayload-Batch due to SdkError.");
                    failures.extend(msg_payload.into_values());
                }
            }
        }

        failures
    }
}
