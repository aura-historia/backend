use crate::payload::MailPayload;
use aws_sdk_sqs::{error::SdkError, operation::send_message_batch::SendMessageBatchError};

#[derive(thiserror::Error, Debug)]
pub enum MailServiceError {
    #[error("Encountered SQS SdkError for SendMessageBatch: {0}")]
    SdkSQSSendMessageBatchError(#[from] SdkError<SendMessageBatchError>),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait QueueMailService {
    async fn queue_mails(
        &self,
        payloads: Vec<MailPayload>,
    ) -> Result<Vec<MailPayload>, MailServiceError>;
}

#[derive(Debug, Clone)]
pub struct QueueMailServiceImpl<'a> {
    sqs_client: &'a aws_sdk_sqs::Client,
}

impl<'a> QueueMailServiceImpl<'a> {
    pub fn new(sqs_client: &'a aws_sdk_sqs::Client) -> Self {
        Self { sqs_client }
    }
}
